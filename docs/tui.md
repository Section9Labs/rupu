# rupu TUI

The TUI is the default view for `rupu workflow run` and the new
`rupu watch <run_id>` command. It renders a live DAG canvas of the
in-flight run with status glyphs and connector lines, and lets you
approve/reject paused gates inline.

> **Note:** Single-agent `rupu run` keeps its line-stream output for
> v0; per-run-directory TUI attach is deferred to v0.1.

## Surfaces

| Command | Behavior |
|---|---|
| `rupu workflow run <wf>` | Spawns the workflow run + attaches the TUI. |
| `rupu watch <run_id>` | Re-attaches to any in-flight or completed run. |
| `rupu watch <run_id> --replay [--pace=N]` | Replays a finished run at N events/sec. |

`q` (or ESC) detaches without affecting the run. The runner keeps going.

## Views

| Key | Action |
|---|---|
| `v` | Toggle Canvas (LTR) ↔ Tree (TTB). |
| `tab` / `shift-tab` | Focus next / prev node. |
| `↑↓←→` or `hjkl` | Pan or focus (depending on view). |
| `enter` | Expand focused node (full transcript pager). |
| `a` | Approve focused gate (`⏸` only). |
| `r` | Reject focused gate (`⏸` only). |
| `f` | Filter: hide completed nodes. |
| `/` | Search node by id. |
| `?` | Help overlay. |
| `q` | Detach. |

## Status palette

| Glyph | Meaning |
|---|---|
| `●` | Active |
| `◐` | Working / streaming |
| `✓` | Complete |
| `✗` | Failed |
| `!` | Error but `continue_on_error` |
| `○` | Waiting |
| `↺` | Retrying |
| `⏸` | Awaiting human approval |
| `⊘` | Skipped (`when:` evaluated false) |

## Environment

- `RUPU_TUI_DEFAULT_VIEW=tree|canvas` — sticky default view per session.
- `NO_COLOR=1` — disable ANSI color (glyphs only).

## Approval inline

When a node enters `⏸`, focus jumps to it and a bottom toast appears:
`⏸ <node>: "<prompt>"  [a] approve  [r] reject  [enter] expand`. Press
`a` to resume the run; press `r` to reject (you'll be prompted for a
reason, max 200 chars).

## Limitations (v0)

- No mouse interaction (deferred to v0.1).
- No editing workflows from the TUI.
- Single-agent `rupu run` not yet TUI-attached (v0.1).
- Cross-emulator polish: tested on iTerm 3.5+ (macOS), Alacritty 0.13+,
  Windows Terminal 1.20+. Other emulators may render glyphs poorly —
  `RUPU_TUI_DEFAULT_VIEW=tree` is the safest fallback.
