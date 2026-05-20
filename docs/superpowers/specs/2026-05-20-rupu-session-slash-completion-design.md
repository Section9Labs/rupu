# rupu-cli session slash-command tab-completion

**Status:** Draft
**Author:** matt
**Date:** 2026-05-20
**Surface:** `rupu-cli` interactive session prompt (`rupu session start <agent>` and `rupu session attach`)

## Problem

Slash commands are the keyboard-driven control surface for a live session
(`/help`, `/status`, `/workflow show-run …`, etc.). Today the only way to
issue one is to remember the exact name and type it from scratch. There is
no completion, no listing, and no discovery loop — `KeyCode::Tab` is
silently ignored in the prompt handler.

This hurts two cases:

1. **Discovery.** A new user doesn't know the catalog. `/help` exists but
   requires committing the prompt first to read the output.
2. **Speed for power users.** Even when the catalog is known, the routed
   commands are long (`/workflow show-run current …`). Typing `/w<Tab>`
   should at least narrow the field.

## Goal

Add bash-style Tab completion on the session prompt, surfaced as a small
in-screen popup so the user can *see* the candidates and the current
selection — matching what other modern CLI tools (Claude Code, Codex,
GitHub CLI in some modes) do.

## Non-goals

- Subcommand completion (`/workflow sh<Tab>` → `show`). Tracked as a
  follow-up.
- Argument completion (workflow names, run IDs, issue refs).
- Fuzzy matching. Strict prefix match only.
- Completion outside the prompt (e.g. on key bindings like `/runs` shown
  in the help row).
- Persisting popup state across detach / re-attach.

## Design

### UX flow

1. User starts typing — `/`, `/h`, etc. No popup yet.
2. **Tab** opens the popup. The popup shows all commands matching the
   current prefix (everything after the leading `/`), with the first
   candidate highlighted.
3. **Tab** again, or **Down**, moves the highlight forward. **Up** moves
   backward. Both wrap.
4. **Enter** while the popup is open: accept the highlighted command
   into the buffer, close the popup, *do not submit*. The user has to
   press Enter a second time to actually run the command. This is the
   safe choice — routed roots (`/workflow`, `/session`, …) need
   subcommands; accepting + submitting would land a parse error.
5. **Esc** closes the popup without modifying the buffer.
6. **Any character, Backspace, Ctrl-U, Ctrl-W** types into the buffer
   normally; if the popup is open, candidates are re-filtered live.
   When the buffer no longer starts with `/`, the popup closes.

When accepting a **routed root** (`workflow`, `session`, `issues`,
`transcript`), the accepted buffer gets a trailing space —
`"/workflow "` — so the user can immediately type the subcommand.
Builtin commands get no trailing space.

### Visual

The popup renders as up to 8 dim rows immediately above the prompt row.
No box-drawing — a single `▸` marker on the highlighted line, `  `
indent on the others. Each row shows the command name and a one-line
description, with the description column padded to align as a table.

```
   /help        show available commands
 ▸ /history     replay prompt history
   /runs        list runs in this session
   /transcript  show the current transcript
issue-reader > /h▏
```

If there are more than 8 candidates, the visible window scrolls with
the highlight and a trailing dim row `↓ +N more` marks the truncation.

### Catalog

Completable commands (in display order — alphabetical):

| name | description | routed |
| --- | --- | :-: |
| `/cancel` | cancel the active run | – |
| `/detach` | leave the live view (keep session running) | – |
| `/help` | show available commands | – |
| `/history` | replay prompt history | – |
| `/issues` | (routed) issues show / list | ✓ |
| `/quit` | quit and stop the session | – |
| `/runs` | list runs in this session | – |
| `/session` | (routed) session show / list | ✓ |
| `/status` | session status detail | – |
| `/stop` | stop the worker | – |
| `/transcript` | show the current transcript | – |
| `/workflow` | (routed) workflow show / list / show-run | ✓ |

Aliases (`h`, `?`, `exit`) are still parsed when typed manually, but
intentionally excluded from completion to keep the list short.

`/transcript` appears once: as a builtin (immediate "show the current
transcript") AND as a routed root (`/transcript show|list`). In the
catalog we keep it as builtin-non-routed so `Enter` doesn't add a
trailing space; users typing `/transcript show …` simply type past the
acceptance. (Trade-off documented; alternative is to mark it routed
and accept that bare `/transcript` users get a stray trailing space —
worse.)

### State

New `SessionInteractiveState` field:

```rust
struct CompletionState {
    query: String,              // the prefix after the leading '/'
    candidates: Vec<usize>,     // indices into SLASH_COMMANDS catalog
    index: usize,               // selected position within `candidates`
    scroll_offset: usize,       // for the 8-row visible window
}

completion: Option<CompletionState>,
```

Module-private catalog (sorted alphabetically by `name`):

```rust
struct SlashCommand {
    name: &'static str,
    description: &'static str,
    routed: bool,
}

const SLASH_COMMANDS: &[SlashCommand] = &[ … ];
```

### Key handling

In `handle_session_live_input`, when `state.prompt_active` is true:

- **Tab**:
  - If `completion` is `None` and the buffer starts with `/`: build the
    candidate list from the current prefix, set `completion =
    Some(CompletionState{ … })` with `index = 0`. If the candidate list
    is empty, no-op (do not open).
  - If `completion` is `Some(_)`: advance `index` (wrap to 0 past last).
    Adjust `scroll_offset` so `index` stays in the visible window.
- **Down** (when popup open): same as Tab.
- **Up** (when popup open): decrement `index` (wrap to last). Adjust
  scroll.
- **Enter** (when popup open): accept the highlighted candidate. Replace
  `input_buffer` with `"/<name>"` plus `" "` if `routed`. Clear
  `completion`. Return `AttachControl::Continue` (do NOT dispatch).
- **Esc** (when popup open): clear `completion`. Buffer unchanged.
- **Char / Backspace / Ctrl-U / Ctrl-W**: existing handling runs first.
  After the buffer mutates, if `completion` was open: rebuild `query`
  + `candidates` + reset `index = 0` and `scroll_offset = 0`. If the
  buffer no longer starts with `/` after the edit, *or* the rebuilt
  candidate list is empty, clear `completion`.

When `prompt_active` is false, Tab does nothing (consistent with the
"letters start a prompt" rule — Tab is not a letter).

### Screen integration

`build_session_screen_rows_for_size` reserves up to 9 rows
(8 candidates + 1 truncation marker) above the prompt row when
`completion.is_some()`. The viewport reduces its content row budget
by that count so the popup never overlaps existing rows. If the
terminal is too small to fit the popup plus 1 content row plus the
prompt, the popup is silently capped further (down to as few as 1
visible candidate).

### Rendering

New helper `render_session_completion_rows(state, width) -> Vec<String>`
returns the popup rows in display order. Description-column padding
is computed from the *visible* candidate set (so widths don't jump
as the user scrolls).

The helper is independent of the prompt-line renderer. The screen
builder concatenates: header + transcript rows + completion rows +
prompt row.

## Components

All changes in `crates/rupu-cli/src/cmd/session.rs`:

- `SLASH_COMMANDS: &[SlashCommand]` — new const.
- `struct CompletionState`, `SessionInteractiveState::completion`.
- `fn slash_completion_open(state, prefs)` — builds initial state from
  current `input_buffer`, returns `bool` (false if no candidates).
- `fn slash_completion_refilter(state)` — recomputes candidates after
  buffer edits; clears state if buffer no longer matches.
- `fn slash_completion_accept(state)` — writes the highlighted
  candidate into `input_buffer`, clears `completion`.
- `fn render_session_completion_rows(state, width) -> Vec<String>`.
- `handle_session_live_input` — new branches for Tab / Up / Down /
  Enter-while-completion / Esc; existing edit branches call
  `slash_completion_refilter` when `completion.is_some()`.
- `build_session_screen_rows_for_size` — reserves rows for the popup
  and inserts the rendered completion rows before the prompt row.

## Testing

Unit tests (added to the existing `tests` module in `session.rs`):

- `slash_completion_filters_by_prefix` — `/` returns all, `/h` returns
  `{help, history}`, `/zz` returns empty.
- `slash_completion_tab_advances_and_wraps` — Tab past last candidate
  resets to index 0.
- `slash_completion_up_arrow_decrements_and_wraps` — Up before index 0
  goes to last candidate.
- `slash_completion_enter_accepts_into_buffer` — buffer becomes
  `/help` (no trailing space) for a builtin.
- `slash_completion_enter_accepts_routed_with_trailing_space` —
  buffer becomes `/workflow ` for a routed root.
- `slash_completion_enter_does_not_dispatch` — `AttachControl::Continue`
  returned; no command executed.
- `slash_completion_esc_closes_without_changes` — buffer unchanged,
  `state.completion` is `None`.
- `slash_completion_typing_refilters` — `/h` then `e` shrinks candidate
  list; `/h` then Backspace expands back.
- `slash_completion_closes_when_buffer_loses_slash` — Backspace past
  the `/` clears `completion`.
- `slash_completion_popup_renders_above_prompt` — screen builder
  output places the popup rows directly above the prompt row, in
  alphabetical order, with the highlight marker on the selected row.

## Risks

- **Visual collision with the agent-block left rail.** The popup
  candidates use dim foreground only — no `│` — so they read as
  distinct from agent-turn content. Manual smoke test required.
- **Tab in nested prompts.** There are no other places that bind Tab
  in the live view today (grep confirmed). Routing all Tab handling
  through the prompt-active branch keeps it localized.
- **Catalog drift.** The catalog must stay in sync with
  `parse_attach_command`. Mitigated by a unit test
  `slash_completion_catalog_matches_parser` that asserts every entry
  in `SLASH_COMMANDS` round-trips through `parse_attach_command` to
  the same parsed variant.

## Acceptance

- Running `rupu session start <agent>` and pressing Tab on a `/`
  buffer opens an in-screen popup listing matching commands with the
  first highlighted.
- Tab / Up / Down navigate; Enter accepts into the buffer (with
  trailing space for routed roots); Esc dismisses.
- Typing characters re-filters live.
- All new and existing session tests in
  `crates/rupu-cli/src/cmd/session.rs` pass.
- No regressions in transcript replay or screen-builder width
  invariants (existing `retained_session_screen_rows_respect_*`
  tests still pass).
