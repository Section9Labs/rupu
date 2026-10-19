# rupu CP — common visual language + app-wide rollout

**Status:** design, approved in direction (mocks reviewed 2026-10-19)
**Surface mode:** Operate (app screens worked in daily)
**Scope:** The rupu Control Plane web UI (`crates/rupu-cp/web`). Pure frontend UI/UX — a dashboard/admin interface. No backend or data-model change beyond what a screen genuinely needs.

## Intent

The CP works, but every screen was styled on its own: three different page-header patterns, the brand purple used as a filled button in exactly one place, bare "Loading…" states, gradient area charts, run-on summary lines, and four different pill/badge styles. There is no shared visual language, so the app reads as assembled rather than designed. This spec defines one language and rolls it across the app as **real shared components**, starting with the Dashboard.

## The language (reference mocks committed alongside this spec)

Open these in a browser — they are the visual contract:
- `assets/cp-visual-language/visual-language.html` — the eight components, each as a before→after, plus the language spelled out.
- `assets/cp-visual-language/dashboard-redesign.html` — the Dashboard before→after, reorganised around triage.
- `assets/cp-visual-language/charts-v2.html` — the **living charts** (approved): a hero "run seismograph" + two animated dashboard tiles, with draw-in motion, a breathing now-pulse, a shared crosshair, and a `prefers-reduced-motion` fallback. (`charts.html` is the earlier static version, kept for contrast.)

The rules, in short (full detail in the mocks):
- **Tokens only.** Every color is an existing CP token (`ink`/`ink-dim`/`ink-mute`, `brand-*`, `sev-*`, `status-*`, `ok`/`err`/`warn`, `surface`/`panel`/`border`). No new raw hex. Static Tailwind class strings. No `any`.
- **Page header** — one component: optional Back, a title at one size (~20px), an optional one-line subtitle, a right-aligned action zone. One primary action per view (near-white solid), secondary bordered, ghost for icons. The brand purple stays on data/identity, never a chrome button.
- **Chips** — one pill shape for status, severity and config-origin: a pale tint of its own hue, dark-on-light text, a dot only for live status.
- **KPIs** — type, not tiles: one divided panel, label over number, the attention value in red; no sparkline on a KPI.
- **Charts** — the living-charts treatment: thin stacked areas that split the measure (outcomes = completed/failed; throughput = by trigger), a real baseline, recessive grid, round ticks above the max, rounded data-ends, a direct-labelled peak, a crosshair+tooltip, purposeful motion with a reduced-motion fallback. Colors validated colorblind-safe.
- **Summary** — a key/value strip, not a run-on sentence; tabular numbers; every relative figure names its baseline.
- **Loading & empty** — a skeleton in the real layout, then an empty state that keeps the frame and says one plain sentence.
- **Microcopy** — sentence case, plain words.

## Rollout (as real shared components)

Build the language as reusable components, then migrate screens onto them — the same spec → plan → subagent-build discipline used for the transcript reader.

1. **Shared components (foundation):** `PageHeader`, `Chip`, `Button` hierarchy, `KpiStrip`, `Chart` (the living-chart engine — SVG + motion + hover crosshair, reduced-motion aware), `SummaryStrip`, `Skeleton`, `Field`. Each tested, each from CP tokens.
2. **Dashboard first** (the top screen): rebuild `Dashboard` on the components, reorganised around triage (a "Needs you" hero, type-first KPIs, fleet host cards, the living charts, the inventory strip), per `dashboard-redesign.html` + `charts-v2.html`.
3. **Then the rest**, screen by screen (lists, Settings, detail pages), each migrating to the shared components.

## Non-goals / anti-goals
- No backend, API, or data-model change; this is rendering/UI only.
- No new nav routes.
- Don't split the language across screens — the point is consistency; a screen that reinvents a header or a chip is a regression.
- Don't trade the richness (type scale, the living charts, full content) for flat restraint.

## Verification
Built with full tests + `tsc` + `npm run build` green, and **looked at in a real browser** (light + dark, desktop + phone, live + completed) — matt's hard rule; green tests are not enough. Each increment is its own PR against main.
