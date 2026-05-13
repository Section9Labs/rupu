# rupu UI Theme System Design

**Date:** 2026-05-13  
**Status:** Accepted / Phase 1 implemented  
**Companion docs:** [Slice C TUI design](./2026-05-05-rupu-slice-c-tui-design.md), [Autoflow observability design](./2026-05-11-rupu-autoflow-observability-design.md)

---

## 1. What this is

A real theme system for `rupu`, not just a syntax-highlighting toggle.

The CLI now has two separate theme concepts:

- **syntax theme** — used for code / markdown / YAML highlighting through `syntect`
- **palette theme** — used for rupu’s own chrome: statuses, tables, frames, ticker, autoflow/session/workflow UI

This split is necessary because a syntax theme and a semantic CLI palette solve different problems.

---

## 2. Problem statement

Before this change, `rupu` had a single `[ui].theme` string that mostly behaved like a syntect theme selector. That was insufficient for the current product surface:

- `workflow run`, `watch`, `autoflow serve`, `session attach`, and table views all use semantic colors, not syntax token colors.
- Operators need palette consistency across statuses (`running`, `blocked`, `complete`), frames, rails, warnings, and labels.
- Users want recognizable named themes (`tokyo-night`, `dracula`, `github-dark`, etc.), not just raw syntax themes.
- We need a path for external theme import rather than hardcoding every palette by hand forever.

---

## 3. Goals

1. Split syntax-theme selection from UI-palette selection.
2. Keep backward compatibility with the legacy `[ui].theme` knob.
3. Ship a small curated set of embedded palette themes.
4. Support local theme files and project-local overrides.
5. Support importing an external theme into rupu’s native schema.
6. Apply the active palette broadly enough that the feature is visible in real command surfaces, not only in config parsing.

Non-goals for Phase 1:

- full remote theme catalog / marketplace
- live preview TUI
- VS Code / iTerm / WezTerm / tmTheme import parity
- per-command ad hoc color overrides

---

## 4. Config model

Recommended config shape:

```toml
[ui]
color = "always"
pager = "never"

[ui.syntax]
theme = "Solarized (dark)"

[ui.palette]
theme = "tokyo-night"
```

Compatibility rule:

- `[ui].theme` remains a legacy catch-all fallback
- when present, consumers may treat it as both:
  - a syntax-theme hint
  - a palette-theme hint

Resolution order:

### Syntax theme
1. command flag (where supported)
2. `[ui.syntax].theme`
3. `[ui].theme`
4. built-in default: `base16-ocean.dark`

### Palette theme
1. `[ui.palette].theme`
2. `[ui].theme`
3. built-in default: `rupu-dark`

---

## 5. Theme storage model

### Global
- `~/.rupu/themes/*.toml`

### Project-local
- `<repo>/.rupu/themes/*.toml`

Project-local themes override global and built-in themes by name.

This gives teams a clean way to ship repo-specific visual language without changing the global install.

---

## 6. Native theme schema

The runtime reads one native schema regardless of import source.

```toml
version = 1
name = "tokyo-night"
description = "Dark palette inspired by Tokyo Night"
base = "rupu-dark"
syntax_theme = "base16-ocean.dark"

[palette]
running = "#7aa2f7"
complete = "#9ece6a"
failed = "#f7768e"
awaiting = "#e0af68"
skipped = "#9aa5ce"
soft_failed = "#ff9e64"
retrying = "#bb9af7"
dim = "#565f89"
brand = "#7aa2f7"
brand_subtle = "#9d7cd8"
tool_arrow = "#7dcfff"
separator = "#3b4261"
sev_critical = "#c0caf5"
sev_high = "#f7768e"
sev_medium = "#ff9e64"
sev_low = "#e0af68"
sev_info = "#7dcfff"
label_palette = ["#f7768e", "#ff9e64", "#e0af68", "#9ece6a"]
```

Properties:

- `version` gates future schema upgrades.
- `base` enables inheritance from a built-in or installed theme.
- `syntax_theme` is an optional hint for the human-facing syntax pairing.
- palette colors are semantic, not ANSI-slot-based.

---

## 7. Embedded themes

Phase 1 ships a curated set of palette themes:

### Native rupu themes
- `rupu-dark`
- `rupu-light`
- `rupu-midnight`

### Mapped presets
- `tokyo-night`
- `dracula`
- `gruvbox-dark`
- `github-dark`
- `github-light`
- `solarized-dark`
- `solarized-light`
- `catppuccin-mocha`

These are intentionally few. The goal is quality and recognizability, not an immediate giant catalog.

---

## 8. External theme system

Phase 1 external support is **import**, not direct runtime dependency on third-party formats.

### Supported now
- native rupu theme TOML
- Base16 themes (local file or URL)

### Why Base16 first
- widely available
- format is stable
- maps well onto semantic CLI palettes
- enough existing ecosystem depth to bootstrap user customization

### Deferred import targets
- VS Code themes
- terminal color schemes (`.itermcolors`, WezTerm, kitty)
- `.tmTheme` / syntax-theme conversion

The runtime should always convert imported themes into the native rupu schema.

---

## 9. CLI surface

Phase 1 command surface:

- `rupu ui themes`
- `rupu ui theme show <name>`
- `rupu ui theme validate <path>`
- `rupu ui theme import <path-or-url> [--from auto|rupu|base16] [--name <name>] [--project]`

Output contract:

- `rupu ui themes` supports `table | json | csv`
- `rupu ui theme show|validate|import` supports `table | json`

---

## 10. Runtime application model

The implementation uses a semantic palette indirection layer:

- historical code still references semantic tokens such as `RUNNING`, `DIM`, `BRAND`, `SEPARATOR`
- render helpers remap those tokens through the active palette at runtime

This matters because it avoids a giant all-at-once rewrite of every printer callsite.

Primary application surfaces in Phase 1:

- `output::palette::Status`
- line printer / ticker
- diagnostics
- YAML snippet renderer
- table status cells and label chips
- any existing callsite going through the shared palette write helpers

Some surfaces will still need incremental cleanup later if they bypass the shared helpers.

---

## 11. Design decisions

### Keep syntax and palette separate
A syntax theme is not enough to style the CLI chrome correctly.

### Use semantic palette keys
The runtime should ask for `running`, `failed`, `brand`, `separator`, not “ANSI blue”.

### Native schema is the source of truth
Imports convert into rupu’s format; the runtime does not grow a pile of format-specific readers.

### Local and project themes before remote catalogs
Theme install from a URL is allowed through import, but a remote marketplace is deferred until the schema and UX settle.

---

## 12. Follow-on backlog

Not part of Phase 1:

- `rupu ui theme preview <name>`
- `rupu ui theme use <name>` convenience command
- VS Code / terminal-theme importers
- downloadable theme catalog / remote install registry
- richer palette coverage for any remaining hardcoded table colors or ANSI literals
- full theme preview snapshots in CI

