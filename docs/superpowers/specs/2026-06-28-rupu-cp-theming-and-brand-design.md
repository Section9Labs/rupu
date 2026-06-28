# rupu-cp — Theming (light + dark) & brand inheritance — Design

**Date:** 2026-06-28
**Surface:** `crates/rupu-cp/web` (the Control Plane UI)
**Status:** approved direction (matt). Decisions locked: **lifted near-black** dark palette; **follow-system + remembered** toggle; **plan now, build after the multi-host CP work lands** (or coordinate) to avoid merge conflicts. Brand inheritance (the rupu ∞ icon + rupu.sh aesthetic) is in scope.

## Goals
1. A **dark theme** for the CP using the rupu.sh palette, alongside the current **light** theme, with a toggle that follows the OS by default and remembers the user's choice.
2. The CP **inherits the rupu.sh brand**: the **∞ rupu mark** in the sidebar + as the favicon (today it's a placeholder violet square; the referenced `/favicon.svg` is missing), and an aesthetic consistent with the site (violet accent, the same identity).
3. Do it **without regressing** the light theme, and pay down color debt by routing colors through tokens.

## Current state (assessed)
- **No** `darkMode`, no ThemeProvider/toggle. `tailwind.config.ts` defines hard-coded hex tokens: `bg`, `panel`, `border`, `ink{.dim,.mute}`, `brand{50..700}`, `sev{…}`.
- **Strong semantic-token usage** (auto-themes once tokens become variables): `text-ink*` ~589, `border-border` ~170, `bg-panel` ~97, `brand-*` ~130.
- **Long tail of raw colors** (won't adapt without migration): `slate-*` **318**, status `red/green/amber/blue-*` **497**, `bg-white` **49**, inline hex **121** (graph `stepStyle.ts`, severity, charts, CodeMirror theme).
- A design-system core already exists (#400 status tokens + UI primitives like `components/ui/Button`; #402 typography/button/chip sweep). New: `CodeEditorImpl` now uses `githubHighlightStyle` from `components/codeHighlightTheme` — the dark plan adds a dark CM highlight beside it.
- The CP brand header (`components/Layout.tsx`) is a placeholder: a `bg-brand-500` rounded square + white dot + "rupu / Control Plane". `web/index.html` references `/favicon.svg` (missing) + title "rupu Control Plane".

## Architecture — CSS-variable tokens (the core decision)
Make the palette **CSS variables** that flip per theme; Tailwind tokens resolve to them, so the ~1,000 semantic-token usages re-theme automatically.

- Store colors as **RGB channels** (not hex) so Tailwind's `/<alpha-value>` opacity modifiers keep working:
  ```css
  :root { --c-bg: 250 250 250; --c-panel: 255 255 255; --c-ink: 15 23 42; … }   /* light (today) */
  [data-theme="dark"] { --c-bg: 10 10 10; --c-panel: 20 20 22; --c-ink: 245 245 245; … }
  ```
  ```ts
  // tailwind.config.ts
  colors: {
    bg:    'rgb(var(--c-bg) / <alpha-value>)',
    panel: 'rgb(var(--c-panel) / <alpha-value>)',
    border:'rgb(var(--c-border) / <alpha-value>)',
    ink:   { DEFAULT:'rgb(var(--c-ink) / <alpha-value>)', dim:'rgb(var(--c-ink-dim) / <alpha-value>)', mute:'rgb(var(--c-ink-mute) / <alpha-value>)' },
    brand: { 50:'rgb(var(--c-brand-50) / <alpha-value>)', …, 700:'rgb(var(--c-brand-700) / <alpha-value>)' },
    surface:'rgb(var(--c-surface) / <alpha-value>)', 'surface-hover':'rgb(var(--c-surface-hover) / <alpha-value>)', // NEW (for slate migration)
    // semantic status tokens (NEW; light+dark), see §Status:
    'ok','ok-bg','warn','warn-bg','err','err-bg','info','info-bg',
  }
  ```
- **Theme switch:** `darkMode: ['selector', '[data-theme="dark"]']` so any leftover `dark:` variants also work if needed.
- **Brand scale, themed semantically.** The numeric `brand-*` scale is used by *role*: `50` = subtle bg, `600` = solid bg, `700` = text. Each `--c-brand-N` gets a dark value so existing classes stay correct: e.g. dark `--c-brand-50` → a dark violet tint, dark `--c-brand-700` → light violet `#a78bfa` (readable on dark). No component churn for brand.

### The two palettes
| token | light (today) | dark (lifted near-black) |
|---|---|---|
| `bg` | `#fafafa` | `#0a0a0a` |
| `panel` | `#ffffff` | `#141416` (lifted) |
| `surface` (new) | `#f1f5f9` (slate-100) | `#1b1b1f` |
| `surface-hover` (new) | `#e2e8f0` | `#232327` |
| `border` | `#e5e7eb` | `rgba(245,245,245,.10)` → channels `245 245 245` w/ alpha utility |
| `ink` | `#0f172a` | `#f5f5f5` |
| `ink-dim` | `#64748b` | `rgba(245,245,245,.62)` |
| `ink-mute` | `#94a3b8` | `rgba(245,245,245,.40)` |
| `brand-600` (solid) | `#6d28d9` | `#7c3aed` |
| `brand-700` (text) | `#5b21b6` | `#a78bfa` |
| `brand-50` (subtle bg) | `#f5f3ff` | `rgba(124,58,237,.16)` |
| status `err/ok/warn/info` (+ `-bg`) | red/green/amber/blue-600 + -50 | tuned for dark (e.g. fg lighter, bg = low-alpha tint) |

(Borders/tints that need alpha use the channel form + an alpha utility, e.g. `border-border` with `--c-border: 245 245 245` and a default 0.10 alpha applied via a dedicated `--c-border` already-alpha approach — finalize in P1.)

## Theme runtime
- **ThemeProvider** (`components/theme/ThemeProvider.tsx`): resolves `theme = stored ?? system`; sets `document.documentElement.dataset.theme`; listens to `matchMedia('(prefers-color-scheme: dark)')` while in "system" mode; persists explicit choice in `localStorage['rupu.cp.theme'] = 'light'|'dark'|'system'`.
- **No-flash:** a tiny inline `<script>` in `index.html` sets `data-theme` from localStorage/system **before** first paint.
- **Toggle:** a control in the top bar (sun/moon, 3-state: light/dark/system or simple light↔dark) using the `Button`/icon primitives; accessible (`aria-pressed`/label).

## Brand inheritance (the ∞ icon + style)
- Add **`web/public/favicon.svg`** = the rupu ∞ mark (the same asset shipped on rupu.sh: dark rounded tile + violet ∞ stroke). Fixes the missing favicon; update `<title>`/theme-color.
- Replace the placeholder logo in **`Layout.tsx`** with a small **∞ mark** (inline SVG or the favicon) + "rupu" wordmark + "Control Plane" sub-label — matching the site's nav brand. Reusable `components/Brand.tsx`.
- Aesthetic: violet accent already matches; the dark theme + ∞ mark make the CP read as the same product as rupu.sh. Keep the CP's denser, data-first layout (don't import the marketing site's giant type).

## Raw-palette migration (the bulk; map → tokens)
- `bg-white` (49) → `bg-panel`.
- `slate-*` (318): `text-slate-{400,500}` → `text-ink-mute`/`ink-dim`; `text-slate-{700,900}` → `text-ink`; `bg-slate-{50,100}` → `bg-surface` (hover → `bg-surface-hover`); `border-slate-200` → `border-border`; `ring-slate-*` → `ring-border`.
- Status `red/green/amber/blue-*` (497) → semantic status tokens (`text-err`/`bg-err-bg`, `ok`, `warn`, `info`) with light+dark values; align with the #400 status tokens + the existing `sev` scale (severity stays its own scale, themed).
- **Inline hex (121):** `stepStyle.ts` `STATE_STYLE`, RunGraph card `bg-white`/`#e5e7eb`, severity, recharts series, CodeMirror theme. Make theme-aware: expose a small `useThemeColors()` (reads CSS vars via `getComputedStyle`) or a per-theme JS map; pass to recharts; the graph nodes switch card bg → `panel`/`surface` tokens; CodeMirror gets a **dark highlight style** beside `githubHighlightStyle` selected by current theme.
- **a11y:** verify WCAG AA contrast for text/ink on both themes; focus rings visible on dark.

## Files (by area)
- `tailwind.config.ts` (token → CSS-var channels; `darkMode`; new `surface`/status tokens), `src/index.css` (the `:root` + `[data-theme=dark]` palettes), `index.html` (no-flash script, favicon, title).
- New: `src/components/theme/ThemeProvider.tsx`, `ThemeToggle.tsx`, `src/components/Brand.tsx`, `public/favicon.svg`, a dark CodeMirror highlight in `components/codeHighlightTheme.ts`.
- Migration touches many `src/**/*.tsx` (neutrals, status, inline hex) — the high-conflict part.

## Phasing (PRs — sequence after multi-host, or coordinate)
- **P1 — Foundation + brand (low conflict).** Tokenize palette to CSS vars (light values identical to today → zero visual change in light), add dark palette, ThemeProvider + no-flash + toggle, and the **∞ favicon + sidebar mark**. After P1: chrome (the ~1,000 semantic usages + brand) themes correctly in dark; raw-palette areas still light-ish. Shippable; visible brand win.
- **P2 — Neutrals.** `bg-white`→`panel`; `slate-*`→`ink/border/surface` (+ the new surface tokens). Mostly mechanical.
- **P3 — Status colors.** `red/green/amber/blue` tints → semantic status tokens (light+dark); reconcile with #400 status tokens + `sev`.
- **P4 — Inline/graph/charts/CodeMirror + a11y.** `stepStyle`/RunGraph/severity inline → theme-aware; recharts palette; dark CodeMirror highlight; contrast audit; final polish.

Each phase is independently mergeable; matt visually validates the binary between phases (per repo rule, GUI rendering is human-verified).

## Non-goals / notes
- Not adding user-authored custom themes (the TUI/app `rupu ui themes` system is separate); just light + dark for the web CP.
- Coordinate with the in-flight multi-host CP work; P1 is low-conflict and could land first, but the migration phases (P2–P4) touch many components and should be sequenced.
