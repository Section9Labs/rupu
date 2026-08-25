# rupu.app macOS — Phase 6: Ambient

**Date:** 2026-08-25
**Status:** Executed under matt's standing "continue with all of the phases" authorization (2026-08-24); decisions recorded for the phase checkpoint
**Parent:** `docs/superpowers/specs/2026-08-20-rupu-macos-app-design.md` (umbrella §8, Phase 6)
**Visual contract:** `docs/macOS_design/V2-CONTRACT.md`; Situation Room intent per HANDOFF artboard 05 + the web's shipped `/events` page (`web/src/pages/Events.tsx` + `components/situationRoom/*` + `lib/situationRoom/*` — the algorithms are the contract)

The ambient layer: the app becomes useful when it isn't frontmost. Two plans:

- **Plan 6A — controls:** full Settings (server config read/write), local notifications, menu bar extra.
  `docs/superpowers/plans/2026-08-25-rupu-macos-phase-6a-settings-notify-menubar.md`
- **Plan 6B — surfaces:** Situation Room scene, code/source viewers, autoflow claims.
  `docs/superpowers/plans/2026-08-25-rupu-macos-phase-6b-situation-code-claims.md`

## 1. Settings (6A)

The Settings scene grows from one tab to four: **General** (exists: appearance; gains nothing new), **Connection** (split out of General: embedded port, binary path, keep-running; remote URL/token entry moves here from onboarding re-entry), **Config** (the server's own configuration — new), **Notifications** (local prefs — new). The umbrella's "Dashboard" tab is **covered by Overview's in-screen Customize menu** (disposition recorded; a fifth Settings tab would duplicate it).

**Config tab** (server-backed, `GET /api/config` + the write routes; all writes disabled with an honest note when the backend is read-only — the server 501s):
- **Effective view**: the resolved config tree with per-key provenance chips (global/project/default — from `provenance`), read-only, searchable.
- **Raw editors**: global TOML and (project-scoped, via a project picker) project TOML — whole-file text editing with monospace, save via `PUT .../global` / `PUT .../project/:id` `{raw}`; server-side validation errors (400 bodies) surface inline; `restart_required` keys from the response surface as a banner. Raw-file writes carry no dotted-key handling client-side (the file IS the text).
- **Policy lock list**: view + edit the global `[policy].lock` entries (`PUT /api/config/policy`). Lock entries are dotted keys — the client implements the **canonical dotted-key encode** (segment containing `.` or `"` → quoted with `\"` escaping, matching `rupu_config::resolve::dotted()` and web `quoteSegment`) as a pure, table-tested function. This is the ONLY dotted-key surface this phase.
- **Deferred (tracked): typed form tabs** (Providers/Autoflow/SCM/Pricing/CP — the web ConfigEditor's 650-line form engine). The Raw + Policy + Effective triad is complete and honest; form ergonomics are post-parity. Typed `patch` writes (and with them the full decode contract) arrive with the forms.

## 2. Notifications (6A)

`UNUserNotificationCenter`, driven by the existing firehose (no new transport): **gates** (`stepAwaitingApproval` — alert, carries run/step/reason), **failures** (`runFailed`/`stepFailed` — alert, carries error), **completions** (`runCompleted` — banner). Per-kind toggles in the Notifications tab (`@AppStorage`, local-only — the CP has no user accounts). Authorization requested on first enable, never at launch. Notifications deep-link to the run detail (`userInfo` → route). A single long-lived notifier task owned by BackendController-adjacent machinery consumes its own firehose stream (the established independent-stream seam); dedup by (runID, kind) within a short window so replay-on-reconnect doesn't spam. **Deferred (tracked): critical-finding alerts** — findings are not on the event wire (web merges them via REST); needs a polling design, post-parity.

## 3. Menu bar extra (6A)

`MenuBarExtra` (SwiftUI): status glyph with an attention dot when `awaiting > 0`. Popover: 4 stat tiles (running / awaiting / paused / pending — `GET /api/dashboard?host=local`'s `active`, polled 60s while the popover machinery is alive, reusing the HostsFooterStore loop idiom), a top-5 needs-you list (reusing `deriveNeedsYou` over a lightweight local-only runs fetch — NOT a full ActivityStore; gates first, inline Approve/Reject through the shared `PendingActions` gate-scoped machinery), and footer buttons (Open rupu / New run / Settings). Everything renders honestly from local-only data (menu bar surfaces must be fast; fleet-wide is one click away in the app).

## 4. Situation Room (6B)

A separate fullscreen scene (View ▸ Enter Situation Room; borderless, **dark always** per artboard 05): `PulseStrip` vitals (active/awaiting from dashboard, findings summary, session-local events-per-minute sparkline) · center **editorial event stream** · right **project roster**. The web's shipped `lib/situationRoom/{cards,roster}.ts` algorithms are ported as pure, tested Swift (card derivation from `CPEvent`+findings with content-identity dedup excluding ts/pos; roster fold with terminal-status reconciliation via the existing `runDetail` lazy resolution). Transport is 100% existing (firehose stream + REST). Event cards deep-link to runs. **Recorded caveat**: the web's own redesign leaves `/events`' long-term standalone status open — the macOS scene commits per the umbrella; if the web later folds it, revisit at the redesign pass.

## 5. Code & source viewers (6B)

- **Project Code tab** (fills Phase 5A's deferred slot in Project detail): lazy file tree (`GET /api/projects/:ws_id/tree`), file viewer (`/source` — mono, line numbers, language label, honest `available:false` states for big/binary files), name-filter over `/files` (truncation flag surfaced).
- **Run-scoped previews in the transcript**: tool-result cards for grep/ast_grep gain inline source slices (`GET /api/runs/:id/source` — soft-fail states honest, incl. `REMOTE_NOT_SUPPORTED` for remote runs) and a CST tree for ast_grep matches (`GET /api/runs/:id/ast`, matched-node highlighting, truncation flag). Port the web's mount points (`ToolCard` equivalents in the transcript feed), not its React internals.

## 6. Autoflow claims (6B)

The Activity screen's `autoflows` kind gains a **Runs / Claims** sub-toggle (mirrors the web's autoflow page tabs): claims table (issue, repo, workflow, status, owner/lease, last error/summary, PR link) over `GET /api/autoflows/claims`, with **Release** (confirm-first, idempotent) and **Requeue** actions through `PendingActions` (immediate-response contract — the endpoints return synchronously). Fleet-wide claim data is local-only (the store roots in the local global dir) — stated honestly.

## 7. Dispositions (umbrella parity ledger)

- `config` (read + write): shipped (Raw/Policy/Effective; typed forms deferred-tracked).
- `workspace` (stage/delta/discard): **intentionally no UI** — it is a host-to-host wire protocol with zero web UI; the app is a CP client, not a host connector. Recorded as n/a-by-design (revisit only if the app ever becomes a connector target).
- `source`, `code`: shipped (viewers above).
- `fs` (browse): **deferred (tracked)** — the native launcher uses NSOpenPanel for local paths ("native feel" wins); `fs/browse` only matters for browsing a REMOTE CP's filesystem, which has no consumer yet.
- `autoflow_claims`: shipped (claims sub-toggle).
- `fleet_inventory`: **already consumed** — it has no HTTP surface; its data rides `GET /api/dashboard`'s `fleet` field, shipped in Phase 4's FleetStrip. Recorded.

## 8. Verification

Fixtures for every newly consumed endpoint (config view, claims, tree/source/files, run source/ast — emitted from real serde types; workspace/fs excluded per dispositions). Pure-function test tables for: dotted-key encode, card/roster/vitals derivations, needs-you menu derivation reuse. Store gates as established (generation guards, client-identity rebuild, stub isolation, condition polling, @MainActor rule). Live GUI validation against matt's real CP at each plan's close (the three Phase 5B bug classes — eager containers, ForEach identity, lazy-tab dispatch — get explicit checks on every new list surface). Checkpoint packages per plan; merge on verified-green CI per the standing directive.

## 9. Out of scope

Typed config forms; critical-finding notifications; fs/browse UI; workspace-protocol UI; web-side changes; the post-phases UI-redesign pass (parked visual nits live in the arc memory).
