# rupu UI Theme System — Plan 1 Foundation

**Date:** 2026-05-13  
**Status:** Implemented  
**Design:** [rupu UI Theme System Design](../specs/2026-05-13-rupu-ui-theme-system-design.md)

---

## 1. Scope

Land the first usable theme system end to end:

- split syntax theme and palette theme in config
- add built-in palette themes
- add native theme-file loading from global and project directories
- add Base16 import support
- add `rupu ui ...` command surface
- apply active palette to shared render helpers and key operator surfaces
- document the model

This plan is intentionally a foundation slice, not the whole long-term theme roadmap.

---

## 2. Deliverables

### A. Config foundation
- add `[ui.syntax].theme`
- add `[ui.palette].theme`
- keep legacy `[ui].theme` as fallback
- extend parse tests

### B. Theme registry + native schema
- define native palette theme schema
- add built-in palette theme registry
- add global / project theme directories
- support `base = "..."` inheritance

### C. Import pipeline
- support import from:
  - native rupu theme file
  - Base16 theme file
  - URL source
- install imported themes into:
  - `~/.rupu/themes/`
  - `<repo>/.rupu/themes/` with `--project`

### D. CLI surface
- `rupu ui themes`
- `rupu ui theme show`
- `rupu ui theme validate`
- `rupu ui theme import`
- wire these into the standard output contract

### E. Palette application
- active runtime palette
- shared semantic token remapping
- table status colors + label chip palette
- line printer ticker / diagnostics / YAML snippet integration

### F. Docs + tests
- design doc
- user docs
- focused CLI tests
- backlog follow-on entry in `TODO.md`

---

## 3. Execution order

### Step 1 — Config split
Create the stable config contract first so later code has one place to read from.

### Step 2 — Runtime palette layer
Introduce the active semantic palette and shared remapping before touching many callers.

### Step 3 — Theme registry and file loading
Add the native schema, built-ins, and directory lookup.

### Step 4 — Import + validation
Add Base16/native import once the registry exists.

### Step 5 — CLI command surface
Expose the feature to users only once listing/show/import actually works.

### Step 6 — Palette adoption cleanup
Patch the important remaining direct-color callsites that bypass the shared helpers.

### Step 7 — Tests and docs
Lock the surface with CLI tests and user-facing documentation.

---

## 4. Validation plan

Minimum validation for this slice:

- `cargo check -p rupu-cli -p rupu-config`
- CLI list/show/validate/import tests for `rupu ui`
- config parse test covering `[ui.syntax]` and `[ui.palette]`
- smoke verification that the active palette affects at least:
  - status cells
  - label chips
  - line-printer ticker

---

## 5. Explicitly deferred

- remote theme marketplace
- theme preview command
- VS Code theme import
- terminal theme import
- per-theme screenshots or snapshot rendering suite
- complete elimination of all historical hardcoded colors across every CLI surface

These remain valuable, but they are follow-on phases rather than blockers for a usable v1 theme system.

