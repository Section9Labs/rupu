# rupu.app macOS — post-parity backlog

**Date:** 2026-08-25
**Status:** Seeded at the Phase 7 (Ship) checkpoint — the macOS umbrella program
(`docs/superpowers/specs/2026-08-20-rupu-macos-app-design.md`) is functionally complete
(seven phases, all umbrella screens real, no placeholders) and carrying a signed/notarized DMG/zip release lane (lane authored and
statically validated; its first live sign→notarize→staple run is the beta cut at the
phase checkpoint — Phase 7 spec §6). This doc collects everything the program deliberately
deferred along the way, in one place, so the next pass — parity gaps first, then the
dedicated UI-redesign pass matt asked for — starts from an honest list instead of
re-discovering these by grep.

**Sweep method:** every row below is sourced from one of the locations the Phase 7 plan
names as the sweep surface:
1. Umbrella §8's phase table (rows 2–6) and the "Phase 2 disposition" / "Phase 3
   disposition" paragraphs immediately below it, in
   `docs/superpowers/specs/2026-08-20-rupu-macos-app-design.md`.
2. `docs/superpowers/specs/2026-08-24-rupu-macos-phase-5-breadth-design.md` — its
   per-screen "Deferred (tracked): …" call-outs (§2, §4) and §5's re-recorded
   dispositions and §6 "Out of scope".
3. `docs/superpowers/specs/2026-08-25-rupu-macos-phase-6-ambient-design.md` — §7
   "Dispositions (umbrella parity ledger)" and §9 "Out of scope".
4. `docs/superpowers/specs/2026-08-25-rupu-macos-phase-7-ship-design.md` §5
   (no-self-update constraint) and §1 (sandboxing).
5. The Phase 7 plan's own backlog-seeding step
   (`docs/superpowers/plans/2026-08-25-rupu-macos-phase-7-ship.md`, Task 4) names two
   items with no earlier phase-spec citation — **claims manual refresh** and
   **offset-keyed sortable tables** — plus points at "the parked visual nits from the
   redesign memory" for the visual-nit rows. That memory pointer resolves to the
   `macos-ui-redesign-deferred` session-memory note (matt, 2026-08-24), which is itself
   the already-distilled collection of PR #598/#599 checkpoint feedback — its five named
   nits are reproduced verbatim below, with no additions beyond what that note lists.

No item below was invented for this doc; every row traces to one of the five sources
above, cited in its own "Disposition source" cell. Items that were opened as gaps but
subsequently **closed** by a later phase (Phase 3's scope-ambiguous-launch bug, fixed
end-to-end by Phase 5; Phase 5's Projects "Code tab", shipped by Phase 6) are not
listed — this is a backlog of what is still open, not a full history.

## Backlog

| # | Item | Origin phase | Disposition source | Notes |
|---|---|---|---|---|
| 1 | `transcripts` (plural — archive/delete mutations) | Phase 2 | Umbrella §8, "Phase 2 disposition" paragraph; re-recorded Phase 5 spec §5 | Mutation-only endpoint; no consumer in Phase 2's read-only scope, and Phase 3's write path didn't pick it up either — still no screen calls it. |
| 2 | `run_resolve` (`GET /api/run_resolve`-style resolved-entity lookup) | Phase 2 | Umbrella §8, "Phase 2 disposition" paragraph; re-recorded Phase 5 spec §5 | Deferred pending a screen that needs resolved-entity display beyond what `/api/runs` already carries; still none does. |
| 3 | `usage-timeline` endpoint | Phase 2 (opened) / Phase 5 (final disposition) | Umbrella §8, "Phase 2 disposition" paragraph; Phase 5 spec §5 | Originally deferred-tracked pending the Usage screen; Phase 5 shipped Usage but left it **intentionally dormant to match web parity** (web doesn't consume it either) rather than wiring it up. Not a gap to close — recorded so the disposition history is visible. |
| 4 | Web CP launcher's scope-ambiguous-name bug | Phase 3 (opened) / Phase 5 (macOS side fixed) | Phase 5 spec §3 | The server-side scope-aware launch fix (`scope_kind`/`scope_id` on the three launch bodies) shipped in Phase 5 and macOS threads it through `LauncherStore`. The web CP launcher was never updated to send the new fields — "tracked follow-up", explicitly not done here (rupu-cp web change, out of this program's scope). |
| 5 | `GET /api/attention` (server-computed needs-you feed) | Phase 4 | Umbrella §8 Phase 4 row | Overview's needs-you queue is a client-side aggregate today (`deriveNeedsYou`, reused by the menu bar extra); a dedicated endpoint remains deferred-tracked. |
| 6 | Overview Customize block drag/reorder edit mode | Phase 4 | Umbrella §8 Phase 4 row | HANDOFF.md specifies drag/reorder for the Customize visibility menu; Phase 4 shipped show/hide persistence only. Explicitly deferred to the post-phases UI-redesign pass. |
| 7 | Project Detail screen "Config tab" (per-project config write, in-context) | Phase 5 (opened) / Phase 6 (partially addressed) | Phase 5 spec §2 (Projects); cross-checked against `ProjectDetailTab` in `apps/rupu-macos/RupuKit/Sources/RupuProjects/ProjectDetailScreen.swift` (no `.config` case) and Phase 6 spec §1 | Phase 5 named both a Code tab and a Config tab as deferred to Phase 6. Phase 6 shipped the Code tab and shipped project-scoped TOML **write** — but only reachable via Settings → Config's project picker, not as an in-context tab on the Project Detail screen itself. The in-context tab is still not built. |
| 8 | Fleet add-host/enroll forms (node / ssh / bucket) | Phase 5 (opened) / Phase 6 (not revisited) | Phase 5 spec §2 (Fleet) | Named "Settings-adjacent, revisit Phase 6"; Phase 6's Settings scene shipped General/Connection/Config/Notifications with no add-host UI. CLI (`rupu host add`) covers it today. |
| 9 | Library structured definition editors (AgentBuilder / WorkflowEditor) | Phase 5 | Phase 5 spec §2 (Library) | Read-only detail views shipped; an authoring surface for agents/workflows is explicitly post-parity backlog. |
| 10 | Library "Used by" reverse-reference links | Phase 5 | Phase 5 spec §2 (Library) | Net-new even relative to web — no server endpoint computes reverse references (which runs/workflows use a given agent). Needs a new API; recorded so Library's umbrella line has an explicit disposition. |
| 11 | Security findings: agent/session-surface rows non-navigating | Phase 5 | Umbrella §8 Phase 5 row | Global findings use `declared_by` for surface-split navigation; rows whose surface is an agent or session don't navigate yet, pending richer per-row data from the server. |
| 12 | Security coverage: audit / gap / runs / diff tabs + coverage templates | Phase 5 | Phase 5 spec §4 (Security) | Web's heaviest coverage machinery; Coverage screen shipped Overview + Catalog only. Explicitly "post-parity backlog with the umbrella's existing template disposition." |
| 13 | Usage: custom drag-select time windows | Phase 5 | Phase 5 spec §4 (Usage); umbrella §8 Phase 5 row | Usage's range is limited to the global toolbar's fixed presets (7d/30d/all) this phase; a custom drag-selectable window was explicitly deferred. |
| 14 | `GET /api/repos` | Phase 5 | Umbrella §8 Phase 5 row (API-modules column); Phase 5 spec §5 | No macOS consumer — Fleet cards don't need it. Deferred-tracked, not a UI gap. |
| 15 | Settings: typed config form tabs (Providers / Autoflow / SCM / Pricing / CP) | Phase 6 | Phase 6 spec §1; §7 dispositions; §9 "Out of scope" | The Raw + Policy + Effective triad shipped and is "complete and honest"; the web `ConfigEditor`'s ~650-line typed form engine (and the full decode contract typed `patch` writes need) is explicitly post-parity. |
| 16 | Critical-finding push notifications | Phase 6 | Phase 6 spec §2; §9 "Out of scope" | Findings aren't on the SSE event wire (web merges them via REST poll); gate/failure/completion notifications shipped, critical-finding alerts need a polling design — deferred. |
| 17 | `fs`/browse module (remote CP filesystem browsing) | Phase 6 | Phase 6 spec §7 dispositions; §9 "Out of scope" | The native launcher covers local paths via `NSOpenPanel` ("native feel" wins); browsing a *remote* CP's filesystem has no consumer yet, so `fs/browse` stays unconsumed. |
| 18 | Situation Room: follow/pin + fresh-highlight interactions | Phase 6 | Umbrella §8 Phase 6 row | The fullscreen wall ports the web's cards/roster/vitals algorithms; the follow/pin and fresh-highlight interaction affordances are explicitly "parked → redesign pass." |
| 19 | Autoflow Claims table: manual refresh only | Phase 6 (behavior) / named at Phase 7 checkpoint | Phase 7 spec §4 (`docs/superpowers/specs/2026-08-25-rupu-macos-phase-7-ship-design.md`, backlog-seeding list); underlying behavior specified in `docs/superpowers/plans/2026-08-25-rupu-macos-phase-6b-situation-code-claims.md`, Task 3 | `ClaimsStore` loads on first tab selection and reloads only after a Release/Requeue mutation succeeds — there is no live/poll auto-update the way runs get via the firehose. No earlier phase spec calls this out as deferred; it's named directly in the Phase 7 spec's backlog-seeding list (§4). |
| 20 | Offset-keyed sortable tables (Library, Usage's OutlierPanel) | Phase 5B (behavior) / named at Phase 7 checkpoint | Phase 7 spec §4 (`docs/superpowers/specs/2026-08-25-rupu-macos-phase-7-ship-design.md`, backlog-seeding list) | Some sortable tables key rows by array offset rather than a stable identity field — fine while the underlying arrays don't reorder underneath the sort, but fragile. Parked for the redesign pass alongside the generic-sortable-list work from Phase 5's groundwork (Phase 5 spec §1). |
| 21 | App self-update mechanism (Sparkle / appcast) | Phase 7 | Phase 7 spec §5 | "The app has no self-update mechanism this phase (it is a CP client updated by downloading a new DMG); recorded, not built. Sparkle/appcast is post-parity." |
| 22 | App sandboxing | Phase 7 | Phase 7 spec §1 | Release build ships unsandboxed, matching the current dev build; "sandboxing is out of scope and recorded." Revisit only if the app ever needs App Store distribution. |
| 23 | Visual nit: pill shape | Design-alignment (Plan 1/2, PR #598/#599) | `macos-ui-redesign-deferred` session memory (matt, 2026-08-24) | SwiftUI `Capsule` pill shape vs. the web's rounded-4 corner radius. |
| 24 | Visual nit: tinted-vs-flat status pills | Design-alignment (Plan 1/2, PR #598/#599) | `macos-ui-redesign-deferred` session memory (matt, 2026-08-24) | `pending`/`cancelled`/`skipped` render tinted in the app; the web renders them flat-neutral. |
| 25 | Visual nit: brand header badge | Design-alignment (Plan 1/2, PR #598/#599) | `macos-ui-redesign-deferred` session memory (matt, 2026-08-24) | Flagged during the #598/#599 checkpoint review; no further detail recorded beyond the name. |
| 26 | Visual nit: "Awaiting approval" wording | Design-alignment (Plan 1/2, PR #598/#599) | `macos-ui-redesign-deferred` session memory (matt, 2026-08-24) | Copy nit on the gate-pending state's label. |
| 27 | Visual nit: scope-select control thinness | Design-alignment (Plan 1/2, PR #598/#599) | `macos-ui-redesign-deferred` session memory (matt, 2026-08-24) | The top-bar project-scope picker reads visually thin next to the web equivalent. |

## Recorded as intentionally out of scope (not backlog — listed for audit completeness)

These were surfaced by the same sweep but are **not** open work — each has a final,
deliberate "won't build" or "already covered" disposition, so they don't belong on a
post-parity punch list:

- **`workspace` (stage/delta/discard)** — intentionally no UI; it's a host-to-host wire
  protocol with zero web UI either (Phase 6 spec §7: "n/a-by-design"). Revisit only if
  the app ever becomes a connector target itself.
- **`repo_scope`** — internal server helper, no HTTP surface (umbrella §8 Phase 5 API
  column: "internal (n/a)").
- **`fleet_inventory`** — already consumed; it has no HTTP surface of its own, its data
  rides `GET /api/dashboard`'s `fleet` field, shipped in Phase 4 (Phase 6 spec §7).
- **`host_fanout`** — corrected at Phase 3/5: there is no `POST /api/host_fanout`
  endpoint. It's an internal server-side dashboard fan-in helper; the Launcher's
  "fan out: all healthy" is client-side (umbrella §8, "Phase 3 disposition" paragraph).
- **Fleet host detail page** — web's version is a thin runs-by-host list; Activity
  already answers the same question, so no dedicated page is planned (Phase 5 spec §2).

## Where this feeds next

Per matt's standing direction (`macos-ui-redesign-deferred` memory): finish all umbrella
phases first (done as of this doc), then run a dedicated UI-redesign pass that starts
from the visual-nit rows above (#23–27) plus whatever the redesign pass itself
surfaces. The remaining rows (#1–22) are parity/feature gaps, not visual ones, and can
be picked up independently of that pass.
