# rupu-cp Theming (light + dark) & brand inheritance — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development.

**Goal:** Add a dark theme (rupu.sh "lifted near-black") alongside the current light theme — system-follow + remembered toggle — and make the CP inherit the rupu brand (the ∞ icon + favicon). Ships as **4 phased PRs**; sequence after the multi-host CP work (P1 is low-conflict and may land first).

**Design:** `docs/superpowers/specs/2026-06-28-rupu-cp-theming-and-brand-design.md` (read §Architecture, §two palettes, §Theme runtime, §Brand, §migration).

**Decisions (locked):** lifted near-black dark palette; default = follow OS, explicit choice remembered; CSS-variable tokenization (RGB channels for `/alpha` support); brand = the ∞ mark from rupu.sh.

**Constraints:** light theme must look **byte-identical** after P1 (same hex, now via vars); no `any`; keep recharts/xyflow/codemirror lazy/chunked; stage specific files; never `-A`/`.rupu/*`; route colors through tokens (no new raw palette). Each phase: `npm test -- --run` + `npm run build` green; matt visually validates.

## Reference (read first)
- `crates/rupu-cp/web/tailwind.config.ts` — current tokens (bg/panel/border/ink/brand/sev).
- `crates/rupu-cp/web/src/index.css`, `web/index.html` (favicon ref + title).
- `crates/rupu-cp/web/src/components/Layout.tsx` — the placeholder brand header to replace.
- `crates/rupu-cp/web/src/components/ui/*` (Button etc.), `components/codeHighlightTheme.ts` (the light CM highlight to pair with a dark one).
- The rupu ∞ favicon asset on the site (`gh-pages` branch `favicon.svg`) — reuse it verbatim.

---

## PHASE 1 — Foundation + brand (low conflict; light theme unchanged)

**Outcome:** tokens are CSS variables (light values identical to today), a dark palette exists, a theme toggle (system-follow + remembered, no flash) works, and the CP shows the ∞ brand. After P1 the app chrome themes correctly in dark; raw-palette areas are addressed in P2–P4.

### Task 1: Tokenize the palette (CSS variables; light = no visual change)
**Files:** `tailwind.config.ts`, `src/index.css`.
- [ ] **Step 1:** In `index.css`, add `:root { … }` with every color as **RGB channels** matching today's hex exactly (light): `--c-bg: 250 250 250; --c-panel: 255 255 255; --c-border: 229 231 235; --c-ink: 15 23 42; --c-ink-dim: 100 116 139; --c-ink-mute: 148 163 184; --c-brand-50..700: <today's channels>; --c-surface: 241 245 249; --c-surface-hover: 226 232 240;` plus status tokens `--c-ok/-bg --c-warn/-bg --c-err/-bg --c-info/-bg` (light = green/amber/red/blue 600 + 50 channels).
- [ ] **Step 2:** Add `[data-theme="dark"] { … }` overriding with the dark palette from the spec's table (bg `10 10 10`, panel `20 20 22`, surface `27 27 31`, ink `245 245 245`, ink-dim/mute via lighter channels, brand-700 → `167 139 250`, brand-50 → a violet tint, border light channels, dark status tints).
- [ ] **Step 3:** In `tailwind.config.ts`, set `darkMode: ['selector', '[data-theme="dark"]']` and replace each color hex with `rgb(var(--c-…) / <alpha-value>)`; add `surface`, `surface-hover`, and the status tokens. Keep `sev` (theme it too if quick, else P3).
- [ ] **Step 4:** `npm run build` + visually confirm light is unchanged (same colors). `npm test -- --run` green. Commit: `feat(cp/web): tokenize palette to CSS variables (light unchanged)`.

### Task 2: ThemeProvider + no-flash + toggle
**Files:** create `src/components/theme/ThemeProvider.tsx`, `src/components/theme/ThemeToggle.tsx`; modify `index.html`, `src/main.tsx` (wrap app), `Layout.tsx` (place the toggle).
- [ ] **Step 1:** `ThemeProvider` — context with `theme: 'light'|'dark'|'system'` + resolved `mode: 'light'|'dark'`. On mount/`change`: read `localStorage['rupu.cp.theme']` (default `'system'`); compute resolved from `matchMedia('(prefers-color-scheme: dark)')`; set `document.documentElement.dataset.theme = mode`; subscribe to the media query while in system mode. Expose `setTheme`.
- [ ] **Step 2:** No-flash inline script in `index.html` `<head>` (before the module script): set `data-theme` from `localStorage['rupu.cp.theme']` or the media query, synchronously.
- [ ] **Step 3:** `ThemeToggle` — a `Button`/icon control (sun/moon; optionally a 3-state system/light/dark) wired to `setTheme`, accessible.
- [ ] **Step 4:** Wrap `<App/>` in `ThemeProvider` (main.tsx); place `ThemeToggle` in the `Layout` top area. Tests: ThemeProvider resolves stored/system + sets `data-theme`; toggle calls setTheme. `npm test`/`build` green. Commit: `feat(cp/web): theme toggle (system-follow + remembered, no-flash)`.

### Task 3: Brand — the ∞ icon + favicon
**Files:** create `src/components/Brand.tsx`, `public/favicon.svg`; modify `Layout.tsx`, `index.html`.
- [ ] **Step 1:** Add `public/favicon.svg` = the rupu ∞ mark (copy the site's `favicon.svg` verbatim: dark rounded tile + violet ∞ stroke). Ensure `index.html` `<link rel="icon" href="/favicon.svg">` resolves; set `<title>rupu — Control Plane</title>` + `<meta name="theme-color">`.
- [ ] **Step 2:** `Brand.tsx` — the ∞ mark (inline SVG, violet, currentColor-friendly) + "rupu" wordmark + optional "Control Plane" sub-label; props for size/sublabel.
- [ ] **Step 3:** Replace the placeholder logo block in `Layout.tsx` (the `bg-brand-500` square + dot) with `<Brand/>`. Verify it reads well in BOTH themes (mark uses brand/ink tokens).
- [ ] **Step 4:** `npm test`/`build` green; main bundle unaffected. Commit: `feat(cp/web): rupu ∞ brand mark + favicon in the Control Plane`.

### Phase 1 verification
- Light theme pixel-unchanged; dark theme themes the chrome (sidebar, panels, text, borders, brand) correctly; toggle persists + follows system; no flash on reload; ∞ brand + favicon present. matt validates the binary.

---

## PHASE 2 — Neutrals migration
Map `bg-white`→`bg-panel` (49); `slate-*` (318) → `ink`/`border`/`surface` tokens per the spec mapping. Mechanical; do area-by-area (pages, then components, then graph chrome) to keep diffs reviewable. Gates green each commit.

## PHASE 3 — Status colors
Map `red/green/amber/blue-*` tints (497) → the semantic status tokens (`ok/warn/err/info` + `-bg`), light+dark; reconcile with #400 status tokens and the `sev` severity scale. Verify findings/severity/run-state colors read correctly in dark.

## PHASE 4 — Inline / graph / charts / CodeMirror + a11y
- `stepStyle.ts` + RunGraph + severity inline hex → theme-aware (a `useThemeColors()` reading CSS vars, or per-theme map); graph node card bg → `panel`/`surface`.
- recharts series/grid/axis colors → theme-aware palette.
- Add a **dark CodeMirror highlight** beside `githubHighlightStyle`; select by current theme in `CodeEditorImpl` + `ExpressionFieldImpl`.
- WCAG AA contrast audit (text/ink, focus rings) on both themes; fix gaps. Final polish.

### Final verification
- `npm test -- --run` + `npm run build` green; recharts/xyflow/codemirror still out of the main chunk. Full click-through in both themes (dashboard, runs + live graph, sessions, workflow editor, agents, findings, coverage). matt validates.
- TODO: note any deferred long-tail color spots.
