# Subprocess netflow capture — Plan 6: the CP views (make it visible)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans. Checkbox steps.

**Goal:** Render the subprocess (bash) socket flows — now in the per-run netflow ledger — in the CP network views: the flow table, the flow detail panel, the topology/timeline/org aggregates, the fidelity badge, and the coverage disclosure. Plus surface the `Capture` ledger line's unavailable/loss state, and link a socket row to the tool call in the transcript. This is the final plan; after it the whole feature is usable end to end in the UI.

**Architecture:** The API `FlowView` already `#[serde(flatten)]`s `FlowRecord`, so the new fields (`process`, `local_addr`, `direction`, and `ctx.tool_call_id`) and `Fidelity::Socket` / `Origin::Subprocess` reach the web with NO API struct change — the endpoints already serve the per-run ledger. Work is: (1) the TypeScript types, (2) component rendering for socket-shaped rows (no method/path/status; show process + transport→ip:port), (3) the explorer aggregates already group by origin key so `subprocess:*` nodes appear — add the badge/legend, (4) a small API addition to expose each run's `Capture` state (the ledger's `read_flows` ignores `Capture` lines, so a new reader surfaces them) for the "capture unavailable / lossy" disclosure, (5) a transcript link from a socket row.

**Tech stack:** Rust (`rupu-cp` API) + TypeScript/React web (vitest). All cross-platform, testable on this Mac. `make cp-web` builds the embedded UI.

**Spec:** `docs/superpowers/specs/2026-10-04-rupu-subprocess-netflow-capture-design.md` §12 (§12.4 disclosure, §12.5 transcript anchor).

## Global Constraints
- **No `unsafe`.** `#![deny(clippy::all)]` (Rust). Web: pass `tsc`/lint + vitest.
- **Honesty:** socket rows must never show a method/path/HTTP status they don't have; the fidelity badge marks them `socket`; the disclosure states what bash capture does and does NOT cover (hostnames, unconnected UDP, detached children, remote-host runs) — do not overclaim.
- **No behavior change to HTTP flows:** existing provider/scm flow rendering is untouched; socket rows are an additional shape keyed off `fidelity === 'socket'` / `origin.kind === 'subprocess'`.
- **Visual check:** matt reviews CP rendering. Component vitest tests are necessary but NOT sufficient; the PR must flag that a visual pass is recommended, and the plan builds the web so screenshots are possible.

## File structure
- `crates/rupu-cp/src/api/netflow.rs` — add a per-run `capture_state` to the run-scoped response (read `LedgerLine::Capture` lines); no change to `FlowView` (flatten already carries the new fields).
- `crates/rupu-cp/web/src/lib/netflow.ts` — `Fidelity` + `'socket'`; `Origin.kind` + `'subprocess'`; `FlowView` + `process?`/`local_addr?`/`direction?`; `ctx.tool_call_id?`; doc updates.
- `components/netflow/FidelityBadge.tsx` — `socket` tone + title.
- `components/netflow/NetflowTable.tsx` — socket-row cells + transcript link.
- `components/netflow/explorer/FlowDetailPanel.tsx` — socket-row detail.
- `components/netflow/ScopeDisclosure.tsx` — coverage wording + capture-state surfacing.
- (topology/timeline/org cards need no logic change; `subprocess:*` origins render via existing origin keying — add only label/legend polish if needed.)

---
### Task 1: web types + FidelityBadge socket treatment

**Files:** `web/src/lib/netflow.ts`, `components/netflow/FidelityBadge.tsx`; vitest in `FidelityBadge` (create a small test if none).

- [ ] **Step 1:** in `lib/netflow.ts`: `Fidelity` union gains `'socket'`; `Origin.kind` union gains `'subprocess'`; `FlowView` (or the shared `FlowRecord` type it extends) gains `process?: { pid: number; name: string }`, `local_addr?: string`, `direction?: 'outbound' | 'inbound'`; `FlowCtx`/ctx gains `tool_call_id?: string`. Update the `Origin` doc comment (it currently lists provider/scm/update/cp/system and explains mcp/webhook absence) to add that `subprocess` IS captured (bash tool). Keep the ordering `coarse < socket < http < full` anywhere fidelity is ranked in TS.
- [ ] **Step 2:** `FidelityBadge.tsx`: add a `socket` entry to `FIDELITY_TONE` (pick a distinct tone, e.g. `violet`/`indigo` — not reusing http's green or coarse's amber) and `FIDELITY_TITLE`: "Socket — process, remote IP:port, bytes and timing observed from the OS socket table; no URL, method or HTTP status." Keep the exported `FIDELITY_TITLE` legend in sync (it feeds the explorer CoveragePopover).
- [ ] **Step 3:** vitest: `FidelityBadge` renders the `socket` label + title; a type-level test or a small render test that a `socket` fidelity doesn't crash. Run `npx vitest run FidelityBadge` (in `crates/rupu-cp/web`).
- [ ] **Step 4:** `npx tsc --noEmit` (or the project's typecheck) clean; commit `cp-web: socket fidelity + subprocess origin types and badge`.

---

### Task 2: socket-row rendering in the table + detail panel

**Files:** `components/netflow/NetflowTable.tsx`, `components/netflow/explorer/FlowDetailPanel.tsx`; their `.test.tsx`.

- [ ] **Step 1: failing tests** with a socket-flow fixture (fidelity `socket`, origin `{kind:'subprocess', name:'curl'}`, `process:{pid:4412,name:'curl'}`, `scheme:'tcp'`, `host:'140.82.116.3'`, `port:443`, `method:''`, `path:''`, `status` absent, `ctx.tool_call_id:'toolu_1'`, `run_id:'run1'`, bytes set):
  - NetflowTable: the row's origin cell shows `curl` (and ideally the pid), the "network"/host cell shows `tcp → 140.82.116.3:443`, the method/path cell and the status cell show `—` (not blank, not a bogus method), and when `run_id` + `ctx.tool_call_id` are present a "transcript" affordance is rendered (a link/button). An HTTP flow row is UNCHANGED (regression guard: an existing http fixture still shows `GET /path` + status).
  - FlowDetailPanel: for a socket flow it shows process name + pid, local_addr, direction, bytes in/out, and does NOT show method/path/status/TTFB rows; for an http flow those rows are unchanged.
- [ ] **Step 2:** run → fail.
- [ ] **Step 3: implement.** Branch rendering on `flow.fidelity === 'socket'` (or `origin.kind === 'subprocess'`). Add a small helper `isSocketFlow(f)`. For the table: the existing `titleValue: (f) => \`${f.method} ${f.path}\`` and the status column must render `—` for socket flows; add the transport→endpoint display (reuse the host/port the row already has). The transcript link target: `/runs/${run_id}?...` anchored to the tool call — use the app's existing run-detail route + a `#call-${tool_call_id}` anchor (Task 4 adds the anchor target; here just render the link). For the detail panel: conditionally render the socket field set vs the http field set.
- [ ] **Step 4:** run → pass; the http regression tests still pass; `tsc` clean. Commit `cp-web: render socket flows in the netflow table and detail panel`.

---
### Task 3: capture-state API + coverage disclosure

**Files:** `crates/rupu-cp/src/api/netflow.rs` (expose per-run capture state), `web/src/lib/netflow.ts` (type), `components/netflow/ScopeDisclosure.tsx` (+ its test).

- [ ] **Step 1 (Rust): read Capture lines.** `read_flows` ignores `LedgerLine::Capture`. Add a reader `rupu_netflow::ledger::read_capture_states(path) -> Vec<CaptureStateLine>` (or reuse a single-pass reader) that collects the `Capture` lines, and have the run-scoped netflow endpoint (`GET /api/runs/:id/netflow`) include a `capture: { state: "active"|"unavailable", reason?: string, notes: [...] }` summary derived from them (latest state wins; collect any loss `note`s). Keep it additive — the flows payload is unchanged. Unit-test the reader against a ledger with an `active` then an `unavailable` Capture line + a flow (flows still read correctly; capture summary reflects the lines). Clippy clean.
- [ ] **Step 2 (web type):** add the `capture` field to the run-netflow response type in `lib/netflow.ts`. Note the internally-tagged shape: the Rust `CaptureState` is `#[serde(tag="state")]`, so JSON is `{"state":"unavailable","reason":"..."}` — read `capture.state` accordingly (the Plan-2 `state.state` nesting note applies only to the raw `LedgerLine::Capture`; the API summary should flatten it to a clean `{state, reason, notes}` — do that flattening in the Rust endpoint so the web reads a simple shape).
- [ ] **Step 3: ScopeDisclosure.** This is the ONE place the covered-surface sentence lives. Update the covered list to include "bash subprocess connections (TCP/UDP, by process, attributed to the tool call)". Update the honest-limits text to add the gaps: no hostnames (IP + ASN only), unconnected UDP has no destination, detached children and sub-poll-interval connections may be missed, remote-host runs' bash flows stay on the host. When the run's `capture.state === 'unavailable'`, surface "subprocess capture unavailable on this run: {reason}"; when there are loss notes, surface them. Keep HTTP/provider/scm wording intact.
- [ ] **Step 4:** vitest for ScopeDisclosure (covered list includes bash; unavailable state renders the reason); `cargo test -p rupu-cp` (the reader + endpoint) + `tsc` + vitest green. Commit `cp: expose per-run capture state and disclose bash coverage + its gaps`.

---

### Task 4: transcript link from a socket flow

**Files:** the transcript tool-card component (`web/src/components/transcript/ToolCard.tsx`) + the run-detail route that renders it; `NetflowTable`'s link target (from Task 2); tests.

- [ ] **Step 1:** give each tool call a stable anchor: `ToolCard` (or its wrapper) renders `id={`call-${call_id}`}` so a URL `#call-<id>` scrolls to it. (Confirm the run-detail page renders the transcript with ToolCards keyed by `call_id`.)
- [ ] **Step 2:** make the NetflowTable socket-row "transcript" link target `/runs/${run_id}#call-${tool_call_id}` (or the app's actual run-detail route), and ensure the run-detail page, on load with that hash, scrolls to the anchored card (a `useEffect` on the hash if one isn't already global).
- [ ] **Step 3: fallback if the anchor is non-trivial.** If wiring a reliable scroll-to-anchor across the transcript's virtualization/lazy-load is more than a small change, instead make the link filter the run's Netflow tab to that `tool_call_id` (a query param the NetflowTable already can filter on, or add a simple client-side filter). Pick the smaller reliable option; record which in the report.
- [ ] **Step 4:** vitest covering the link target (the socket row links to the right run + anchor/filter); `tsc` + vitest green. Commit `cp-web: link a socket flow to its tool call in the transcript`.

---

## Final verification
- [ ] `cargo test -p rupu-cp` green; `cargo clippy -p rupu-cp --all-targets` clean.
- [ ] In `crates/rupu-cp/web`: `npx tsc --noEmit` clean; `npx vitest run` green (all netflow component tests incl the new socket-row + disclosure + badge tests).
- [ ] `make cp-web` builds the embedded UI without error (so a release carries the new UI).
- [ ] **Visual check (matt / optional controller browser pass):** with a run whose ledger has a socket flow (the Plan-5 live test produces one), the netflow table shows a `curl` socket row with `tcp → ip:port`, a socket fidelity badge, a working transcript link, and the disclosure lists bash coverage. The PR flags this for matt's visual review; the controller may drive the built-in browser against a local `cp serve` to screenshot it.

## Done = feature complete
After Plan 6 the subsystem is end-to-end: Linux + macOS backends capture an agent's bash connections, attributed to the run/agent/tool-call, written to the per-run ledger, and shown in the CP network views with honest fidelity + coverage disclosure. Remaining deferred items (remote-host bash-flow transport to the coordinator, the detached-child sweep, short-connection capture, watcher perf) are tracked for follow-up arcs.
