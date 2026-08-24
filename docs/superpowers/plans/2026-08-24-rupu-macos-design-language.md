# rupu.app Design Alignment — Plan 1: Design Language Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild RupuDesign as a faithful port of the CP v2 design system — tokens (both themes), sans/mono type discipline, lucide iconography, web chrome kit — and mechanically re-skin every existing screen, so the app *looks* like the web CP before Plan 2 makes it *move* like it.

**Architecture:** Token values ported verbatim from `crates/rupu-cp/web/src/styles.css` (both themes) into dynamic NSColor providers; a 9-state `StatusTone` replaces the 5-tone `RunTone` collapse; lucide icons are extracted one-time from the web's pinned `lucide-react` package into committed SVGs + a generated Swift path-data table rendered by a small tested SVG-path parser (stroke-based, template-tinted — no third-party code); a chrome kit (StatusPill/Button/Badge/Chip/panel/tint-banner) mirrors the web components; a final sweep migrates every screen off SF Symbols, mono-abuse, and old radii.

**Tech Stack:** SwiftUI (macOS 14+), Swift 6, Swift Testing; one-time Node extraction script (committed) reading the existing `web/node_modules/lucide-react@^0.468`.

**Spec:** `docs/superpowers/specs/2026-08-24-rupu-macos-design-alignment-design.md` · **Contract:** `docs/macOS_design/V2-CONTRACT.md` (authoritative values)

## Global Constraints

- Geometry: 4px grid; panel radius **7**, inner card 6; **no shadows**; 1px borders. Icon sizes: 15–16 nav, 9–11 in pills.
- Typography: **sans (system) for ALL UI text**; **mono ONLY for data** (ids, timestamps, numerals, code). Scale: meta 10 / note 11 / ui 12 / lead 13 pt. Eyebrow (mono 9–10pt uppercase tracked .08–.15em, mute) for true captions only.
- Tokens are the styles.css values verbatim (both themes) — the token table in Task 1 is the contract; a transposed value is an Important defect.
- Status is 9-state (`running done failed awaiting paused pending skipped cancelled rejected`; rejected shares failed's color); `ActivityStatus` maps 1:1, `.unknown` renders as pending.
- No third-party Swift code. Lucide assets ship with an ISC license note. No new npm installs — extraction reads the EXISTING `crates/rupu-cp/web/node_modules/lucide-react` (run `npm ci` in `crates/rupu-cp/web` first if absent).
- Store layer untouched: the full suite (242 Swift + 28 Rust) passes throughout; only token-value tests and view files change.
- Null discipline unchanged (`—` never 0). `make macos-test` + `make macos-build` gates; matt side-by-side checkpoint at the end. Never bare `git stash pop`.

## Token table (verbatim from styles.css — Task 1 implements, tests assert)

Neutrals/brand/semantic (light | dark, RGB):
bg 250 250 250 | 10 10 10 · panel 255 255 255 | 20 20 22 · surface 241 245 249 | 27 27 31 · surfaceHover 226 232 240 | 35 35 39 · surfaceActive 203 213 225 | 46 46 51 (fill-only) · border 229 231 235 | 38 38 42 · borderStrong 203 213 225 | 63 63 70 (line/fill-only) · ink 15 23 42 | 245 245 245 · inkDim 100 116 139 | 161 161 170 · inkMute 148 163 184 | 113 113 122 · brand50 245 243 255 | 33 23 56 · brand100 237 233 254 | 45 33 74 · brand500 124 58 237 | 124 58 237 · brand600 109 40 217 | 124 58 237 · brand700 91 33 182 | 167 139 250 · err 220 38 38 | 248 113 113 · errBg 254 242 242 | 43 22 22 · ok 22 163 74 | 74 222 128 · okBg 240 253 244 | 18 40 27 · warn 217 119 6 | 251 191 36 · warnBg 255 251 235 | 48 36 12 · info 37 99 235 | 96 165 250 · infoBg 239 246 255 | 20 32 54.
Severity (+bg): critical 147 51 234 | 168 85 247, bg 250 245 255 | 42 28 56 · high 220 38 38 | 248 113 113, bg 254 242 242 | 48 24 24 · medium 234 88 12 | 251 146 60, bg 255 247 237 | 48 32 18 · low 202 138 4 | 250 204 21, bg 254 252 232 | 46 40 16 · info 100 116 139 | 148 163 184, bg 248 250 252 | 30 31 35.
Status: running 59 130 246 | 96 165 250 · done 34 197 94 | 74 222 128 · failed 239 68 68 | 248 113 113 · awaiting 245 158 11 | 251 191 36 · paused 6 182 212 | 34 211 238 · pending 148 163 184 | 113 113 122 · skipped 203 213 225 | 82 82 91 · cancelled 100 116 139 | 161 161 170 · rejected = failed.

## File structure

```
RupuKit/Sources/RupuDesign/Tokens.swift          # Rewrite: full v2 token set + StatusTone
RupuKit/Sources/RupuDesign/Typography.swift      # Rewrite: meta/note/ui/lead + dataMono + Eyebrow
RupuKit/Sources/RupuDesign/Icons/LucideIconData.swift  # Generated path-data table (committed)
RupuKit/Sources/RupuDesign/Icons/SVGPathParser.swift   # Pure parser (TDD)
RupuKit/Sources/RupuDesign/Icons/Icon.swift            # Icon view (stroke render, template tint)
RupuKit/Sources/RupuDesign/Chrome/{StatusPill,Buttons,Badge,TintBanner}.swift + PanelStyle update
apps/rupu-macos/scripts/extract-lucide.mjs       # One-time generator (committed)
apps/rupu-macos/RupuKit/Sources/RupuDesign/Icons/svg/*.svg + LICENSE-lucide.txt
(Task 5 modifies every view file in RupuActivity/RupuRunDetail/RupuLauncher/RupuShell)
```

---

### Task 1: Tokens v2 + StatusTone

**Files:**
- Rewrite: `RupuKit/Sources/RupuDesign/Tokens.swift`
- Modify: `RupuKit/Sources/RupuStore/ActivityRow.swift` (only the `ActivityStatus.tone` accessor)
- Test: `RupuKit/Tests/RupuDesignTests/TokensTests.swift` (regenerate table-driven, both appearances, EVERY token above)

**Interfaces:**
- Consumes: existing `dynamicColor(light:dark:)` helper pattern (rewrite to accept RGB triples: `dynamicColor(_ l: (UInt8,UInt8,UInt8), _ d: (UInt8,UInt8,UInt8))`).
- Produces (later tasks compile against):
  - `public extension Color`: `rupuBg, rupuPanel, rupuSurface, rupuSurfaceHover, rupuSurfaceActive, rupuBorder, rupuBorderStrong, rupuInk, rupuDim, rupuMute, rupuBrand50, rupuBrand100, rupuBrand, rupuBrand600, rupuBrand700, rupuErr, rupuErrBg, rupuOk, rupuOkBg, rupuWarn, rupuWarnBg, rupuInfo, rupuInfoBg` (`rupuBrand` = brand500; keep the existing name so call sites survive).
  - `public enum StatusTone: String, CaseIterable, Sendable { case running, done, failed, awaiting, paused, pending, skipped, cancelled, rejected }`; `Color.status(_ tone: StatusTone)`.
  - `Severity` keeps its cases; `Color.severity(_:)` re-valued; NEW `Color.severityBg(_:)`.
  - **Compatibility shim**: `RunTone` stays with `@available(*, deprecated, message: "migrate to StatusTone (Task 5)")` and `Color.status(_ t: RunTone)` maps run→running, done→done, fail→failed, waiting→awaiting, pause→paused — deleted in Task 5.
  - `ActivityStatus.tone: StatusTone` maps 1:1 (pending→pending, running→running, completed→done, failed→failed, awaiting→awaiting, rejected→rejected, cancelled→cancelled, paused→paused, unknown→pending). Its old `RunTone` accessor removed; the ONLY RupuStore edit this task is that accessor (call sites of `.tone` keep compiling because they pass it to `Color.status` — which now takes StatusTone; any that break are fixed here minimally and re-skinned properly in Task 5).

- [ ] **Step 1: Regenerate TokensTests** — table-driven `[(name, color, light(r,g,b), dark(r,g,b))]` covering EVERY token in the plan's table (neutrals, brand×5, semantic×8, severity×5+bg×5, status×9), resolved per appearance exactly like the existing test helper; plus `ActivityStatus.tone` 1:1 mapping test incl. unknown→pending, rejected→rejected.
- [ ] **Step 2: RED** (values differ / symbols missing) → rewrite Tokens.swift from the table → **GREEN** (`make macos-test`; expect Task-5-owned view files to still compile via the shim — if any call site hard-breaks, patch minimally and note it).
- [ ] **Step 3: Commit** — `feat(macos-design): v2 token set (both themes) + 9-state StatusTone`

### Task 2: Typography v2

**Files:**
- Rewrite: `RupuKit/Sources/RupuDesign/Typography.swift`
- Test: extend `RupuKit/Tests/RupuDesignTests/` (a small TypographyTests asserting size constants; MicroLabel behavior test if present adapts)

**Interfaces:**
- Produces: `public extension Font { static let metaText: Font /* system 10 */; static let noteText: Font /* 11 */; static let uiText: Font /* 12 */; static let leadText: Font /* 13 */; static func dataMono(_ size: CGFloat) -> Font /* monospaced, monospacedDigit */ }`; `public struct Eyebrow: View { public init(_ text: String) }` — mono 10pt uppercase kerning 1.2 `.rupuMute` (the ONLY sanctioned uppercase-tracked element); `MicroLabel` becomes `@available(*, deprecated)` typealias/wrapper of Eyebrow (deleted in Task 5); `PanelStyle` radius changes to **7** (inner 6) — shadow already absent.
- `Fmt` untouched.

- [ ] **Step 1: Tests** (constants + Eyebrow renders text uppercase — logic-level assertions only) → RED → implement → GREEN.
- [ ] **Step 2: Commit** — `feat(macos-design): v2 type scale (sans UI / mono data), Eyebrow, 7pt panel radius`

### Task 3: Lucide icons — extraction, parser, Icon view

**Files:**
- Create: `apps/rupu-macos/scripts/extract-lucide.mjs`
- Create (generated, committed): `RupuKit/Sources/RupuDesign/Icons/svg/*.svg` (~30 files), `Icons/svg/LICENSE-lucide.txt` (ISC text + attribution), `Icons/LucideIconData.swift`
- Create: `RupuKit/Sources/RupuDesign/Icons/SVGPathParser.swift`, `Icons/Icon.swift`
- Test: `RupuKit/Tests/RupuDesignTests/SVGPathParserTests.swift`, `LucideIconDataTests.swift`

**Interfaces:**
- Produces:
  - `public enum LucideIcon: String, CaseIterable, Sendable { case layoutDashboard, activity, sparkles, workflow, repeatIcon = "repeat", messageSquare, folderGit2, shieldCheck, shieldAlert, network, bookMarked, server, dollarSign, settings, radio, play, checkCircle2, xCircle, pause, pauseCircle, xOctagon, ban, skipForward, clock, arrowLeft, archive, trash2, gitBranch, listOrdered, fileText }`
  - `LucideIconData.paths(for: LucideIcon) -> [String]` (each a full SVG path `d` string in a 24×24 viewBox).
  - `public struct SVGPath { public init?(d: String); public func cgPath(in rect: CGRect, viewBox: CGFloat) -> CGPath }` — supports M/m L/l H/h V/v C/c S/s Q/q T/t A/a Z/z (lucide uses the full set incl. arcs).
  - `public struct Icon: View { public init(_ icon: LucideIcon, size: CGFloat = 16, weight: CGFloat = 2) }` — strokes every path (round cap/join, `weight` scaled by size/24), `foregroundStyle`-tintable.
- Extraction: `node apps/rupu-macos/scripts/extract-lucide.mjs` reads `crates/rupu-cp/web/node_modules/lucide-react/dist/esm/icons/<kebab-name>.js` for each icon (kebab list mirrors the enum), parses the element array (`[["path",{d}], ["circle",{cx,cy,r}], ["rect",...], ["line",...], ["polyline",{points}], ["polygon",...]]`), converts every primitive to path data JS-side (circle→two arcs; rect→M/H/V/Z with rx arcs when present; line→M/L; polyline/polygon→M/L…(Z)), writes both the standalone .svg files (24×24, `fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"`) and `LucideIconData.swift` (static string table). One-time; regeneration documented in the script header.

- [ ] **Step 1: Parser TDD** — failing tests: known simple paths (`"M3 12h18"` → 2 points), curves, an arc command (`A` used by e.g. `clock`'s circle conversion), relative commands, `Z` closure; invalid `d` → nil. RED → implement → GREEN.
- [ ] **Step 2: Run the extractor** (npm ci in web first if node_modules absent); commit generated files. LucideIconDataTests: every enum case has ≥1 path; every path parses via SVGPathParser (round-trip integrity for the whole set).
- [ ] **Step 3: Icon view** (no unit test; verified in Task 5 visuals). `make macos-test` green. **Commit** — `feat(macos-design): lucide icon set — extractor, committed assets, path parser, Icon view`

### Task 4: Chrome kit

**Files:**
- Create: `RupuKit/Sources/RupuDesign/Chrome/StatusPill.swift`, `Chrome/Buttons.swift`, `Chrome/Badge.swift`, `Chrome/TintBanner.swift`
- Test: `RupuKit/Tests/RupuDesignTests/StatusPillTests.swift` (descriptor logic only)

**Interfaces:**
- Produces:
  - `public struct StatusDescriptor { public let tone: StatusTone; public let label: String; public let icon: LucideIcon }` + `public static func descriptor(for: StatusTone) -> StatusDescriptor` — labels/icons per web `status.ts`: running/Play/"Running", done/CheckCircle2/"Completed", failed/XCircle/"Failed", awaiting/Pause/"Awaiting approval", paused/PauseCircle/"Paused", pending/Clock/"Pending", skipped/SkipForward/"Skipped", cancelled/Ban/"Cancelled", rejected/XOctagon/"Rejected".
  - `public struct StatusPill: View { public init(_ tone: StatusTone, compact: Bool = false) }` — dot (or 9–11pt icon) + label, tone color at 12% fill / 30% ring, mono label per web pill convention.
  - `public struct RupuButtonStyle { public static var primary/outline/dangerOutline/ghost: some ButtonStyle }` — primary = brand500 fill white text; outline = 1px borderStrong + ink; dangerOutline = err border/text; ghost = no border, surfaceHover on hover; heights 24–28.
  - `public struct Badge: View { init(_ text: String, tone: Color = .rupuMute) }` (mono meta, tinted 12%); `public struct TintBanner<Content: View>: View { init(tone: Color, toneBg: Color, @ViewBuilder content:) }` — 1px tone/30% border + toneBg fill, radius 7, padding 16/12.
- [ ] **Step 1: descriptor table test** → RED → implement all four files → GREEN; `make macos-build`. **Commit** — `feat(macos-design): chrome kit — StatusPill, button styles, Badge, TintBanner`

### Task 5: Mechanical re-skin sweep

**Files:**
- Modify: every view file in `RupuKit/Sources/{RupuShell,RupuActivity,RupuRunDetail,RupuLauncher}` (+ any RupuDesign leftovers)
- Delete: deprecated `RunTone`, `MicroLabel` wrapper
- Test: existing suites adapt where they referenced RunTone/MicroLabel names

**Interfaces:** consumes Tasks 1–4; produces the fully re-skinned app.

The sweep, file by file (verify each with grep afterward — zero occurrences of the banned patterns):
- [ ] **Step 1: StatusTone migration** — every `Color.status(.run/.done/.fail/.waiting/.pause)` and `.tone` call site moves to the 9-state API (Activity row dots, run-detail pills → replace hand-rolled pills with `StatusPill`; graph glyph colors keep their NodeState mapping but source colors from StatusTone: running→running, done(success)→done, done(!success)→failed, gatePending→awaiting, pending→pending, skipped→skipped). Delete RunTone.
- [ ] **Step 2: Icon migration** — every `Image(systemName:)` in the four modules → `Icon(...)` with the contract mapping (sidebar per V2-CONTRACT nav table; status glyphs via StatusDescriptor; back chevrons → `Icon(.arrowLeft)`; overflow archive → `.archive`). Grep `systemName` → 0 in these modules.
- [ ] **Step 3: Typography sweep** — labels/buttons/section headers/empty-states move to `metaText/noteText/uiText/leadText` sans; `MicroLabel` uses split: true captions (section eyebrows like "TRANSCRIPT", "RUN FACTS") → `Eyebrow`; everything else (button labels "LIVE"/"OFFLINE" pill text, empty-state sentences, banner text) → sans `Text` at the right scale with normal case. Ids/timestamps/numerals/costs stay `Fmt` + `dataMono`. Delete MicroLabel. Grep `MicroLabel` → 0.
- [ ] **Step 4: Chrome sweep** — gate/error banners → `TintBanner`; approve/reject/launch/send buttons → RupuButtonStyle variants (Approve=primary(ok-tinted per web text-button convention: use ok fill), Reject=dangerOutline, Launch=primary, Cancel=outline); panels pick up radius 7 automatically via PanelStyle; hand-rolled 8/999 radii and any `.shadow(` → grep to 0.
- [ ] **Step 5: Gates + visuals** — `make macos-test` (full suite green; adapt renamed-symbol tests), `make macos-build`, `make macos-run` screenshot both themes for the checkpoint package. **Commit** — `feat(macos-design): re-skin all screens to the v2 language`

### Task 6: Checkpoint package + docs

**Files:**
- Modify: `CLAUDE.md` (visual-contract pointer: HANDOFF→V2-CONTRACT; one-line design-language note), umbrella spec companion-documents block (same pointer)
- Test: full gates

- [ ] **Step 1: Docs edits; full gates (`make macos-test && make macos-build && cargo test -p rupu-cp`).**
- [ ] **Step 2: Checkpoint evidence** — app + web side-by-side screenshots (dark + light) of Activity and Run detail; annotate remaining deltas that belong to Plan 2 (flows) so matt's checkpoint judges language only. **Commit** — `docs(macos-design): contract pointers + Plan 1 checkpoint evidence`

---

## Self-review notes

- Spec §2 coverage: tokens→T1, typography→T2, icons→T3, chrome→T4, re-skin→T5, checkpoint→T6. Geometry (7pt radius) lands in T2 (PanelStyle) + T5 sweep. Status 9-state + ActivityStatus 1:1 → T1.
- Compatibility strategy: RunTone/MicroLabel deprecated shims (T1/T2) keep the tree compiling until T5 deletes them — no big-bang breakage between tasks.
- Type consistency: `StatusTone`/`LucideIcon`/`StatusDescriptor`/`Eyebrow`/`RupuButtonStyle` names used consistently across T1–T5.
- Known risk, accepted: SwiftUI cannot render raw SVG — hence the parser+path approach; arcs (A command) must be implemented (lucide circles convert to arcs), covered by dedicated parser tests.
