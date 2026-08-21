# rupu.app — native macOS SwiftUI Control Plane (umbrella spec)

**Date:** 2026-08-20
**Status:** Approved (matt, 2026-08-20)
**Companion documents:**
- `docs/macOS_design/HANDOFF.md` — visual/interaction contract from the Claude Design mockups (tokens, window model, per-screen layouts, widget model). This spec defers to it for all visual detail, with the amendments in §2.
- `docs/macOS_design/rupu.app macOS.dc.html` — interactive prototype.

This is the **umbrella architecture spec** for a multi-phase program. Each phase gets its
own implementation plan (and its own spec where the phase carries real design decisions),
following the same arc pattern as CP Shell v2.

## 1. Goal

A native macOS app (`rupu.app`) that reaches **1:1 functional parity with the `rupu cp
serve` web UI**, then grows beyond it. It connects to an already-running CP instance or
starts one itself. The Rust side is reached **only** via the CP HTTP/SSE API — no FFI.

The existing GPUI-based `rupu-app` crate is **replaced immediately**: it is deleted in
Phase 1 (see §10) and this app becomes the only desktop client.

## 2. Amendments to HANDOFF.md

HANDOFF.md remains the visual contract, with two overrides agreed during design:

1. **No bundled binary.** Embedded mode uses the *installed* `rupu` from PATH (with a
   Settings override), not a copy shipped inside the .app. rupu updates via its own
   channel (`rupu update`); the app version-gates against the server (§5).
2. **Attach-or-spawn.** Before spawning, embedded mode probes the configured port; if a
   CP already answers, the app attaches to it instead of starting a second one.

## 3. Stack & repo layout

- SwiftUI app shell, macOS 14+. AppKit interop where needed (NSVisualEffectView sidebar,
  NSStatusItem menu bar extra, NSPanel palette).
- Vanilla SwiftUI + `@Observable` models. **No third-party architecture framework** (no
  TCA). Third-party dependencies require explicit justification in a phase plan.
- Lives in the rupu repo:

```
apps/rupu-macos/
  project.yml           # XcodeGen — the only committed project definition
  App/                  # thin app target: scenes, wiring, Info.plist, entitlements
  RupuKit/              # local Swift Package — all real code lives here
    Sources/RupuAPI/        # endpoints, Codable models, SSE client, auth
    Sources/RupuBackend/    # process manager, remote config, Keychain
    Sources/RupuStore/      # @Observable app state, event reduction
    Sources/RupuDesign/     # tokens, chrome components, type styles, formatters
    Sources/Rupu<Feature>/  # one module per screen, added per phase
    Tests/                  # headless swift test suites
  Fixtures/             # golden JSON emitted by rupu-cp contract tests (checked in)
```

- `.xcodeproj` is **gitignored**; `make macos-gen` (XcodeGen) regenerates it. New Make
  targets: `macos-gen`, `macos-build`, `macos-test`.
- CI: a macOS lane running `swift test` on RupuKit (headless) + `xcodebuild build` on the
  app target.
- **Thin-app-target rule** (mirrors the "rupu-cli is thin" rule): the app target contains
  only scene declarations and dependency wiring. All logic, state, and views live in
  RupuKit modules. Feature modules depend on RupuAPI/RupuStore/RupuDesign; never on each
  other without a stated reason.

## 4. Module responsibilities

| Module | Owns | Depends on |
|---|---|---|
| RupuAPI | `CPClient` typed endpoints, Codable response/request models, `EventStreamClient` (SSE), Bearer auth injection | Foundation only |
| RupuBackend | `BackendMode`, embedded process lifecycle, PATH discovery, health state machine, Keychain access, version gate | RupuAPI (health probe) |
| RupuStore | App-level state (scope/range, route, backend health), per-screen stores (snapshot + delta reduction), `WidgetConfig` persistence | RupuAPI, RupuBackend |
| RupuDesign | Color/type token catalog (adaptive light/dark per HANDOFF table), panel/pill/micro-bar chrome, null-discipline formatters | SwiftUI only |
| Rupu\<Feature\> | One screen's views + its screen store | RupuAPI, RupuStore, RupuDesign |
| App target | `@main`, WindowGroup / Settings / MenuBarExtra / Situation Room scenes, wiring | RupuKit |

## 5. Backend manager

`BackendMode` = `.embedded(port)` | `.remote(url, tokenRef)` — chosen at onboarding
(artboard 03), changeable in Settings.

**Embedded:**
- Discovery order: login-shell `which rupu` → standard paths (`/opt/homebrew/bin`,
  `/usr/local/bin`, `~/.local/bin`) → Settings override path.
- Probe the configured port (default 7420) with `GET /api/host/info`; on answer →
  **attach** (never tear down an attached server). Otherwise **spawn**
  `rupu cp serve --port <port>` in its own process group.
- "Keep running when window closes" setting controls whether app quit tears down a
  *spawned* child.
- **Version gate:** the app declares a minimum supported rupu version;
  `/api/host/info` reports the server's. Mismatch → blocking banner with a
  `rupu update` hint. Never silent degradation.
- Health state machine: `starting → healthy → degraded → down`, drives the sidebar
  footer status and a Restart button in Settings.
- `rupu` missing entirely → onboarding/Settings shows an install hint; embedded mode is
  unavailable, remote still works.

**Remote:** URL + Bearer token; token stored in Keychain, referenced by `tokenRef`.
Per-host freshness is tracked per host — never one global "live" (HANDOFF rule).

## 6. Data flow

- **REST:** `URLSession` async/await; every endpoint a typed method on `CPClient`.
  Scope/range (`@AppStorage` app-level state) applied to every query that accepts them.
- **SSE:** `EventStreamClient` parses `text/event-stream` from `URLSession.bytes`;
  exponential-backoff auto-reconnect. Consumers: global events stream
  (`/api/events/stream`), run streams, transcript tail. On reconnect a store
  **re-snapshots via REST, then resumes deltas** — no gap-guessing.
- **Stores:** each screen store owns snapshot (REST) + delta (SSE) reduction; views only
  observe. Mutations (Approve/Reject, cancel/pause/resume) use the **pending-state
  contract** (amended by the Phase 3 spec, 2026-08-21): a POST's 200 means *recorded*
  (approve is marker+sweep; cancel/pause travel by signal to a detached `runner_pid`), so
  the UI shows pending until the observed status transition confirms the effect — never
  an optimistic flip.

## 7. Contract tests (drift defense)

- A fixture-emitter test in `rupu-cp` serializes representative instances of every
  response type the app consumes into `apps/rupu-macos/Fixtures/*.json` (checked in).
  A make target regenerates; Rust CI fails on drift between code and checked-in fixtures.
- Swift tests decode every fixture with the RupuAPI models. A Rust-side type change
  therefore breaks a test at CI time, not the app at runtime.
- Fixtures are added per phase, alongside the endpoints that phase consumes.

## 8. Phases

Each phase ends runnable and visually validated (§9). API-module column is the parity
ledger — a module is "covered" when every endpoint in it the web UI uses is reachable
from the app.

| Phase | Delivers | API modules covered |
|---|---|---|
| 1 Foundation | XcodeGen scaffold, RupuDesign tokens, window/sidebar/toolbar shell + routing, onboarding, backend manager, CPClient + SSE + fixture rig; **rupu-app deletion** | `host_info`, `events` |
| 2 Read path | Activity table (kind/status filters, live tail, saved views), Run detail (step graph, transcript feed, netflow + findings rails), Session detail | `runs`, `run_streams`, `transcript`, `transcripts`, `sessions` (read), `graph`, `netflow` (per-run), `findings` (per-run), `run_resolve` |
| 3 Write path | Launcher sheet (agent run / session / workflow, host chips + fan-out), Approve/Reject everywhere gates appear, cancel/pause/resume, session send, archive/restore | `agents` (run/session), `workflows` (run/validate), `host_fanout`, run mutations, `sessions` (send), `tools` |
| 4 Dashboard | Overview widgets read-only → edit mode → WidgetConfig persistence (UserDefaults JSON per HANDOFF) | `dashboard` |
| 5 Breadth | Projects, Security (findings/coverage/catalog), Library, Fleet, Usage screens | `projects`, `coverage`, `findings` (global), `agents`/`workflows`/`autoflows` (definitions), `hosts`, `workers`, `usage`, `usage_outliers`, `repos`, `repo_scope` |
| 6 Ambient | Menu bar extra, ⌘K palette, Situation Room, UNUserNotificationCenter notifications, full Settings (incl. config read + write), workspace delta/stage/discard, source/code viewers | `config` (read + write), `workspace`, `source`, `code`, `fs`, `autoflow_claims`, `fleet_inventory` |
| 7 Ship | CI release lane, signing + notarization, DMG/zip distribution; post-parity backlog seeded | — |

**Parity checklist discipline:** API surface not in the prototype (e.g. coverage
templates, `agents/generate`, `workflows/generate`, node connect) gets an explicit
disposition at the owning phase's planning time: *shipped*, *deferred (tracked)*, or
*intentionally CLI-only*. No silent gaps.

**Phase 2 disposition** (`docs/superpowers/specs/2026-08-20-rupu-macos-phase-2-read-path-design.md`,
complete): `runs`, `run_streams`, `transcript` (singular — `GET /api/transcript` +
`/stream`), `sessions` (read), `graph`, `netflow` (per-run), `findings` (per-run) —
shipped. `transcripts` (plural — archive/delete mutations) — deferred (tracked);
mutation-only, no consumer in the strict-read-only Phase 2 scope, lands with Phase 3
write-path mutations. `run_resolve` — deferred (tracked); no consumer in the
strict-read-only Phase 2 scope (spec §1.1), revisit when a screen needs
resolved-entity display beyond what `/api/runs` already carries. `usage-timeline`
— deferred (tracked); belongs to the Usage screen, Phase 5.

## 9. Error handling, loading, validation

- Per-block store state: `content | empty | loading | failed(error, retry)`. Panels paint
  chrome immediately and fill per block; never blank on one slow host.
- Backend-down is a distinct app-level state (banner + sidebar footer), not a per-screen
  error.
- Null discipline enforced by RupuDesign formatters: unknown renders `—` never 0; partial
  sums marked `+`.
- **No silent-noop paths:** a control that is not wired to a real endpoint does not
  render.
- Testing: headless `swift test` (store reduction, SSE parser, fixture decoding,
  formatters) + Rust fixture drift check. Snapshot tests deferred until pain proves the
  need.
- **GUI validation rule (carried over from rupu-app):** build + test green ≠ rendering
  green. Claude self-verifies with computer-use screenshots; matt runs the app before any
  UI-affecting PR merges.

## 10. rupu-app retirement (Phase 1)

- Delete `crates/rupu-app`. Delete `crates/rupu-app-canvas` **iff** nothing else consumes
  it (verify at plan time; per CLAUDE.md only rupu-app's graph view reads it today).
- Remove their CI lanes, workspace members, CLAUDE.md sections (including the GPUI/Metal
  Toolchain prerequisites and the `cx.defer` rules), and any docs that present rupu-app
  as current.
- rupu-app's unique value (graph view, launcher sheet, approvals) is re-delivered by
  Phases 2–4 of this program.

## 11. Out of scope (this program)

- iOS/iPadOS companion.
- Server-side per-user dashboard persistence (HANDOFF notes it as "later").
- Any new CP API endpoints beyond what parity requires — gaps found mid-phase become
  rupu-cp issues/PRs, specced separately.
- Post-parity "even more" features: collected in the Phase 7 backlog, not designed here.
