# rupu-cli session role-block rendering

**Status:** Draft
**Author:** matt
**Date:** 2026-05-20
**Surface:** `rupu-cli` interactive session view (`rupu session start <agent>` and `rupu session attach`)

## Problem

In the current session UI, `assistant` and `you` rows render as bold-colored role
labels with their body indented two spaces. Tool calls (and every other
nested event) render with a status-glyph prefix (`◐`, `●`, `✓`, etc.) and a
tree-branch (`├─` / `└─`) connector.

Three issues:

1. **No visual differentiation between turns.** The agent's prose and the
   user's prose share the same indent and surrounding rows. When the
   transcript scrolls, it's hard to spot where one turn ends and the next
   begins.
2. **`assistant` label is inconsistent.** Throughout the rest of rupu we call
   these things "agents" — agent files live in `.rupu/agents/`, the CLI is
   `rupu agent`, and the launcher dispatches an "agent". The session row label
   is the last surviving "assistant" in the UI.
3. **Role headers don't carry the status glyph.** Tool calls, gate
   requests, command runs, and notice rows all lead with a `Status::glyph()`.
   Agent and user rows skip it, breaking the visual grammar.

## Goals

- Each agent / user turn renders as a visually distinct **block** without
  relying on terminal background colors.
- Standardize the role header so it carries the same status-glyph as the
  rest of the grid.
- Rename the displayed label `assistant` → `agent` everywhere in the session
  UI.

## Non-goals

- No changes to on-disk transcript schema. `TranscriptEvent::AssistantMessage`,
  `TranscriptEvent::AssistantDelta`, and `SessionEntry::Assistant` keep their
  names — they're storage types, replay-stable, and used by `rupu transcript`
  rendering elsewhere.
- No background-color painting. Considered (see Alternatives) and rejected
  for theme portability + wrap-edge raggedness.
- No changes to the launcher / rupu-app session view (GPUI). Scope is the
  terminal session screen only.

## Design

### Visual

Each agent turn renders as:

```
●  agent  ·  gpt-5  ·  ⇡ 2,311  ⇣ 246  ⟳ 0
│  Sure — checking the file now.
│  Looks like the parser strips the trailing slash on line 42.
├─ ◐ tool read_file  ·  crates/foo/bar.rs
│     {"path": "crates/foo/bar.rs"}
└─ ✓ tool result  ·  3.2 KB
```

Each user turn renders as:

```
▸  you
│  Can you also look at the writer side?
```

Three new visual primitives compared to today:

1. **Status glyph prefix on the role header.** Agent uses
   `Status::Active.glyph()` (`●`) when the turn is complete, or
   `Status::Working.glyph()` (`◐`) while streaming — same convention as a
   tool call. User uses a `▸` glyph in `BRAND` color (no status semantics —
   it just stands in for "incoming message").
2. **Left rail on body lines.** Body lines get a `│ ` prefix in the same
   color as the header glyph. Two characters wide, so total indent
   (`│ ` = 2 chars) matches today's `"  "` indent exactly. No wrap math
   changes.
3. **Tool-call nesting bridges into the rail.** The first nested child of
   an agent turn uses `├─` (existing behavior). The rail's `│` aligns with
   the `│` of the tree continuation prefix (`"  │  "`), so the agent's body
   rail and the nested tool-call tree read as one continuous "this turn"
   container.

### Color rules

| Role | Header glyph | Rail color | Header text |
| --- | --- | --- | --- |
| agent (idle / complete) | `●` `Status::Active` | `Status::Active` | bold `Status::Active` |
| agent (streaming) | `◐` `Status::Working` | `Status::Working` | bold `Status::Working` |
| you | `▸` `BRAND` | `BRAND` | bold `BRAND` |

Color choices reuse existing palette constants (`palette::BRAND`,
`Status::Active.color()`, `Status::Working.color()`). No new colors.

### Label rules

- Header label: `agent` (was: `assistant`). User label stays `you`.
- Role tags emitted by `render_role_header` are the only place affected.
  Internal types, transcript event names, and JSONL on-disk format stay as
  `Assistant`/`AssistantMessage`/`AssistantDelta`.

### Streaming behavior unchanged

The existing logic — `streaming: bool` on `SessionEntry::Assistant` driving
`Status::Working` vs `Status::Active` color — continues to work. The rail
just inherits whichever color the header is using on that frame.

### Width / wrapping

`render_indented_body_lines` already wraps to width with a configurable
prefix. We pass `"│ "` (with the ANSI color escape applied to the `│`) as
both `first_prefix` and `continuation_prefix`. Width math is unchanged:
`│ ` is 2 visible columns, same as the current `"  "`.

Truncating `truncate_ansi_line` already skips ANSI escape bytes when
counting visible width, so coloring the `│` does not corrupt the
truncation point.

## Components

All changes are in `crates/rupu-cli/src/cmd/session.rs`:

- **`render_role_header`** — gains a `glyph: Option<(char, Rgb)>` parameter.
  Renders `<glyph> <label>` when set, falls back to today's bold-label-only
  behavior when `None` (used by call sites we don't want to touch in this
  pass — `SessionActivity::Thinking` live header keeps its current form).
- **New helper `render_role_body`** — wraps body lines with a colored `│ `
  rail. Takes the body lines, the rail color, and the width.
- **`render_session_entry_rows`** — `SessionEntry::UserPrompt` and
  `SessionEntry::Assistant` arms updated to:
  1. Call the new `render_role_header` with the role's glyph + color.
  2. Replace `render_indented_body_lines(... "  ")` with
     `render_role_body(...)`.
- **`render_session_live_status_rows`** — when activity is `Thinking`, the
  live header uses the new agent label `agent` and the `◐` glyph (today it
  passes `"assistant"` with `Status::Working` color, no glyph).

Tests touched (existing assertions on `"assistant"` substring): every test
in the file that asserts `row.contains("assistant")` now asserts
`row.contains("agent")`. Affected tests (from current grep):

- `session.rs:6590`, `:6619`, `:6647`, `:6711`, `:6822`, `:6823`, `:6842`
- Snapshot strings at `:6243`, `:6475`, `:6497`, `:6500`, `:6507`,
  `:6573`, `:6613`, `:6800`, `:6801`, `:6829`, `:6904`, `:6907`, `:6917`

New tests:

- A test that an agent body row starts with the rail `│ ` (ANSI-stripped).
- A test that a user body row also starts with `│ ` in BRAND tint.
- A test that an agent turn followed by a tool call keeps the existing
  `├─` connector AND the agent's body rail reads through to it (visual
  spot-check via row capture).
- A test that the role header includes the status glyph (`●` complete,
  `◐` streaming).

## Alternatives considered

1. **Background-color highlight on body lines.** Rejected: light vs dark
   terminal themes clash; wrapped lines have ragged bg edges; copy-paste
   pulls trailing spaces; we don't paint bg anywhere else in the grid.
2. **Background-color on header strip only.** A real option (used by `gh`,
   Linear CLI) — the section label gets bg, body stays plain. Rejected for
   v1 to keep theme portability. Could revisit later as an optional config
   `[ui].session_section_background = on|off`.
3. **Horizontal `──── agent ────` divider above each turn.** Rejected:
   doubles vertical space per turn; less compact than a rail.
4. **Foreground tinting only (status quo + glyph + rename).** This is
   essentially Option A *without* the rail. Rejected: matt's stated need is
   visual block differentiation, not just a label rename. The rail is the
   minimum change that delivers that without bg-color drawbacks.

## Risks

- **Visual collision with tool-call tree glyphs.** The agent body rail is
  `│` and tool-call continuation is also `│ │`. Mitigated by color: the
  agent rail uses Active/Working color, the tool-call rail uses the
  status-glyph color of the tool-call event (often different). Manual
  smoke test required.
- **Existing assertions on `"assistant"` substring.** Mechanical fix —
  enumerated in Components. No semantic change.

## Acceptance

- Running `rupu session start <agent>` against an existing agent and
  asking a question shows:
  - `●  agent  ·  …` (or `◐  agent  ·  …` while streaming) header.
  - Body lines prefixed with a colored `│ ` rail.
  - User prompts render as `▸  you` with a BRAND-tinted rail.
- All session tests in `crates/rupu-cli/src/cmd/session.rs` pass with the
  updated assertions.
- No changes to on-disk transcript files or replay behavior in
  `rupu transcript`.
