# Handoff: rupu.app — native macOS Control Plane

> **SUPERSEDED (2026-08-24).** This document was the visual contract for Phases 1–3.
> The authoritative visual contract is now `docs/macOS_design/V2-CONTRACT.md`
> (alignment to the CP v2 design system, per
> `docs/superpowers/specs/2026-08-24-rupu-macos-design-alignment-design.md`).
> Kept for the historical baseline; do not build new UI against it.

Implement a Swift macOS app covering all `rupu cp serve` functionality. The visual spec is
`rupu.app macOS.dc.html` (interactive HTML prototype — open in a browser; click the sidebar,
toggle Edit on Overview, ⌘K, click Activity rows). This document is the implementation contract.

## Stack

- **SwiftUI** app shell (macOS 14+), AppKit interop where needed (NSVisualEffectView for the
  sidebar, NSStatusItem for the menu bar extra, NSPanel for the palette).
- One target `rupu.app`. The Rust side is reached ONLY via the CP HTTP/SSE API
  (`crates/rupu-cp/src/api/*`) — no FFI. Backend modes:
  1. **Embedded** (default): the app bundles the `rupu` binary and manages a
     `rupu cp serve --port 7420` child process (launchd agent optional, "keep running when
     window closes" setting). Health-check + restart from Settings.
  2. **Remote**: URL + token, stored in Keychain. Chosen at onboarding (artboard 03).
- SSE for live data (`api/events.rs`, `api/run_streams.rs`); REST for lists and actions.
  Per-host freshness stays per host — never one global "live" for SSH/tunnel transports.

## Design tokens (adaptive light/dark; resolve via semantic colors, never literal hex)

| Token | Dark | Light | Swift |
|---|---|---|---|
| bg | #0A0A0A | #FAFAFA | `Color.rupuBg` |
| panel | #141416 | #FFFFFF | asset catalog, both appearances |
| surface / hover / active | #1B1B1F / #232327 / #2E2E33 | #F1F5F9 / #E2E8F0 / #CBD5E1 | |
| border / strong | #26262A / #3F3F46 | #E5E7EB / #CBD5E1 | strong = lines/fills only, never text |
| ink / dim / mute | #F5F5F5 / #A1A1AA / #71717A | #0F172A / #64748B / #94A3B8 | mute = dimmest legal text |
| brand / brand-hi | #7C3AED / #A78BFA | #7C3AED / #6D28D9 | accentColor = brand |
| run/done/fail/await/pause | 60A5FA/4ADE80/F87171/FBBF24/22D3EE | 3B82F6/16A34A/DC2626/D97706/0891B2 | |
| sev crit/high/med/low/info | A855F7/F87171/FB923C/FACC15/94A3B8 | 9333EA/DC2626/EA580C/CA8A04/64748B | |

Tinted fills = color at 10–15% opacity; rings at 30–35% (see prototype `color-mix` values).
Type: SF Pro for prose/labels; **SF Mono for every identifier, status, micro-label and numeral**
(9–11.5px, uppercase tracked 0.10–0.14em for micro-labels; `monospacedDigit()` on all numerals).
Panel radius 8, inner card 6, pill 999. No shadows on panels — 1px borders only.

## Window model

- **Main window** 1440×900 min 1150×760, full-height sidebar (216pt, NSVisualEffectView
  .sidebar material tinted to token `side`), traffic lights inline. Sidebar sections:
  Overview · **Runs** (Activity = all executions, plus Agent runs / Workflows / Autoflows /
  Sessions leaves that open Activity pre-filtered by kind — sidebar selection and the
  Activity kind-segmented control are the same state) · **Subjects** (Projects) ·
  Security, Library, Fleet, Usage. Footer: host status.
  Settings is NOT in the sidebar → standard Settings scene (⌘,).
- Toolbar (48pt): screen title · project-scope picker · range (7d/30d/all) · ⌘K search field ·
  live pill · theme toggle · **Edit** (Overview only). Scope+range are app-level state
  (`@AppStorage`), applied to every screen.
- **Menu bar extra** (artboard 02): NSStatusItem `∞` with attention-count dot; popover =
  needs-you top items with inline Approve/Reject, 4 stats, footer hotkeys.
- **Situation Room** (artboard 05): borderless full-screen scene (View ▸ Enter Situation Room),
  big numerals + fleet + event wall, dark always.
- **Command palette**: ⌘K NSPanel — entity search (runs, projects, agents, findings) +
  actions (approve gate, run workflow). Esc closes.

## Screens (all in prototype)

1. **Overview** = editable dashboard (below).
2. **Activity** — single execution list: kind segmented control, additive status chips,
   live-tail switch, "+ save view". Table: Status·Kind·Subject·Project·Host·Trigger·Dur·Cost·Started.
   One flexible column (Subject); gated rows get a 4% await tint; row → Run detail.
3. **Projects** — cohort segmented (active/all/dormant); per-row micro-bars for
   Success/Findings/Coverage; findings figure takes worst-severity color; dormant rows dim, `—` cells.
4. **Security** — tabs findings/coverage/catalog; summary strip (coverage %, gaps, severity
   figures); findings table with 2px severity left edge per row.
5. **Library** — tabs agents/workflows/autoflows/contracts; definitions only, never run lists;
   "Used by" links into Activity pre-filtered. Permission badge tone: read-only=done,
   ask=await, bypass=fail (always loud).
6. **Fleet** — host cards (auto-fill 268pt min) + workers table. Faulted card: fail border,
   inset 3px fail edge, err-bg cause panel, Reconnect/Logs.
7. **Usage** — spend/tokens/cost-per-run strip + spend-by-project bars + outlier callout.
8. **Run detail** — breadcrumb + status + facts; Approve/Reject when gated; horizontal step
   graph (done ✓ / gate pulsing / pending dashed); transcript feed (agent prose, tool JSON
   blocks, gate rows with accent left edges); rails: run facts, per-run network (netflow,
   unexpected hosts loud), per-run findings.
9. **Launcher** (toolbar "+ New run", ⌘N — the write path for Runs) — macOS sheet mirroring
   `AgentLauncherSheet.tsx` / `LauncherSheet.tsx`: kind segmented (agent run / session /
   workflow); definition picker (model + scope shown); prompt textarea (agent/session) or
   declared-inputs/kv rows (workflow); target picker (project + branch, fresh worktree clone);
   mode segmented read-only/ask/bypass (same tone rule as Library permissions); **host select**
   as chips with live load (n/slots), faulted hosts disabled, plus a "fan out: all healthy"
   option (`api/host_fanout.rs`). POST = same endpoints as `rupu run` / `rupu workflow run` /
   session start; on success navigate to the new Run detail / Session. Footer shows est. cost
   + queue position.
10a. **Onboarding** — embedded vs remote choice; tokens → Keychain.
10b. **Settings** — General/Connection/Providers/Notifications/Dashboard toolbar tabs;
    appearance System/Light/Dark; launch at login; menu bar toggle; backend status + restart.

## Editable dashboard (Overview)

- **Model**: `[WidgetConfig]` = `{id, size ∈ {S,M,L,W}, order, visible}` on a 6-column grid
  (S=1, M=2, L=3, W=6 columns), persisted as JSON in `UserDefaults` (later: server-side per user).
- **Widget types** (all in prototype): needs-you queue (default W), stat tiles — active now,
  success rate, failed 30d (sparkline), open findings, spend, coverage (default S each),
  outcomes chart, throughput chart, live stream, hosts rail (default L), active-runs table
  (default W, a filtered saved view), custom saved-view widget (hidden by default; any
  Activity filter set can be saved as a widget).
- **Edit mode**: toolbar Edit ⇄ Done. In edit: gallery bar (add hidden widgets, reset layout),
  widgets show a dashed chrome bar — grip ⠿ (drag to reorder = swap), S/M/L/W segmented,
  ✕ remove — plus a subtle jiggle. Implement with SwiftUI `LazyVGrid` + drag targets that
  swap `order`; respect Reduce Motion.
- Needs-you Approve/Reject call the same endpoints as `rupu workflow approve|reject`
  (incl. `--gate <step-id>`); resolve optimistically with a fade, roll back on failure.
- Null discipline: unknown counts render `—`, never 0; partial sums marked `+`.

## Interactions

Hover: rows bg 60%, cards raise border only. Motion: gate/run pulses + live beacons, all
guarded by Reduce Motion. Loading: panels paint chrome immediately, fill per block; never
blank on one slow host. Notifications (UNUserNotificationCenter): gates=alert,
failures/critical=alert, completions=banner (per Settings).

## Sequence

1. App shell: window + sidebar + toolbar, token asset catalog, routing.
2. Backend manager: embedded process lifecycle + remote client + Keychain; onboarding.
3. Activity table + Run detail (shakes out the API client + SSE).
4. Overview widgets read-only → edit mode → persistence.
5. Security, Fleet, Projects, Library, Usage.
6. Menu bar extra, palette, Situation Room, Settings, notifications.