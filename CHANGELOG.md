# Changelog

## v0.5.1 — friendlier diagnostics + auth-login stall fix (2026-05-07)

### Added
- **`output::diag` canonical diagnostic surface.** New `error / warn / info / success / skip / fail` helpers across the CLI replace ad-hoc `eprintln!("rupu <subcmd>: {e}")` patterns at 19 sites. Visual: glyphs (`✗ ⚠ ℹ ✓ ⊘`) + semantic colors that reuse the Okesu palette already shared by the line-stream printer + TUI. Honors `NO_COLOR`, `--no-color`, and `[ui].color = "never"`; falls back to bracketed labels (`[error]`, `[skipped]`) when color is off.
- **`rupu auth status` colored table.** Green ✓ valid, yellow ✓ expiring soon (under 7d), red ✗ expired with the re-login hint inline, dim — for not configured.

### Fixed
- **`rupu auth login --provider <p>` no longer stalls silently.** The default `--mode api-key` path used to read from stdin via `read_to_string` until EOF — without a piped key or `--key`, the command appeared to hang forever. Now: in tty, prints a one-line prompt asking the user to paste + Ctrl-D (and surfaces the `--mode sso` alternative inline when the provider has one); in pipe / heredoc / CI, silent slurp behavior is preserved.

### Changed
- **Default tracing level → `WARN`.** Internal observability (`credential backend = file …`, `github: no credentials configured; skipping connector`) no longer leaks into the user's terminal as if it were CLI output. Opt back in via `RUPU_LOG=info` / `RUPU_LOG=debug` (or any standard `tracing-subscriber` filter directive). User-facing equivalents now route through `output::diag`.
- **`rupu repos list` skip pattern.** `(skipped github: no credential — run …)` parens-style → two-line glyph + indented hint shape via `diag::skip`.

## v0.5.0 — issue-tracker integration + workflow triggers + UI polish (2026-05-07)

A larger release: full issue-tracker integration in the CLI, workflow event triggers (cron-polled + webhook), syntax-highlighted output, shell completion, and a comprehensive UI pass on the listing commands.

### Added
- **Issue-tracker CLI surface** — `rupu issues list | show | run`. Auto-detects the target repo from the cwd's git remote (mirrors `gh issue list`); supports `--label foo` (repeatable, AND match) and `--labels foo,bar` (CSV form).
- **Issue context binding** — when the run-target resolves to an issue (`rupu workflow run <wf> github:owner/repo/issues/42`), the orchestrator pre-fetches the issue payload and binds it as `{{ issue.* }}` in every step's prompt + `when:` filter. The textual ref is persisted on `RunRecord.issue_ref` so `rupu workflow runs --issue <ref>` can filter back.
- **`notifyIssue: true` workflow flag** — auto-comments on the targeted issue at run completion with the run id + outcome.
- **Workflow triggers spec + Plan 1 + Plan 2** — `docs/superpowers/specs/2026-05-07-rupu-workflow-triggers-design.md` + `…-plan-1-polled-events.md`.
  - **Plan 1: polled events on cron tick.** `rupu cron tick` now also polls SCM connectors for events between ticks. New `[triggers].poll_sources` config, deterministic `evt-<wf>-<vendor>-<delivery>` run-ids for idempotency, new `--skip-events` / `--only-events` flags, new `rupu cron events` inspection command. Lets users without a server use event triggers from a one-line crontab entry.
  - **Plan 2: glob matching + extended vocab.** `trigger.event: github.issue.*` syntax. Polled GitHub vocabulary brought to webhook-tier parity (label / assign / edit / review_requested / review_submitted / ready_for_review / synchronize variants).
- **`rupu agent edit` / `rupu workflow edit`** — shell-out to `$VISUAL` / `$EDITOR` (or `--editor "code --wait"`). Project shadow wins by default; `--scope global|project` overrides. Post-edit frontmatter / YAML re-validation (warn-only).
- **`rupu completions` subcommand** — bash / zsh / fish / powershell. Two modes: dynamic bootstrap (default) calls back into rupu at completion time so agent + workflow names are filled from `.rupu/{agents,workflows}/` plus the global counterparts; static (`--static`) prints a self-contained `clap_complete::generate` script. `install` writes to canonical paths.
- **syntect-driven syntax highlighting + pager** for `rupu agent show` and `rupu workflow show`. Frontmatter highlighted as YAML, body as Markdown. New `[ui]` section in `config.toml` (`color` / `theme` / `pager`); `--no-color`, `--theme`, `--pager` / `--no-pager` flags; `NO_COLOR` env var honored.
- **Colored table output** across `*-list` commands (`issues list`, `workflow runs`, `agent list`, `cron list`, `cron events`, `repos list`). Status cells get semantic colors; label cells render as colored chips.
- **Real label colors** — GitHub via `octocrab::models::Label.color`; GitLab via a per-project `/projects/:id/labels` cache (TTL 5 min). New `Issue.label_colors` field. Falls back to a deterministic hash-based palette when upstream colors are absent.
- **Branded OAuth callback page** — replaces the bare "Authentication complete — return to your terminal." with a centered, full-viewport page (#0a0a0a bg, large `∞` glyph, "Don't code it, rupu it." tagline).

### Fixed
- `rupu issues list` empty-state on stdout (was `eprintln!` — broke piping).
- `rupu auth logout --all` no longer hangs on `read_line` in non-tty (CI / scripts).
- `rupu workflow runs` empty results print before the column header instead of after.
- `rupu cron events` empty-state gives actionable guidance + warns when `[triggers].poll_sources` is empty.
- `run not found` errors carry hint lines pointing at `rupu workflow runs` (or `--status awaiting_approval` for approve / reject).

### Changed
- `RunStore::create` now returns `RunStoreError::AlreadyExists` on duplicate id (used by the polled-events tier for idempotent dispatch). Manual `run_<ULID>` ids never collide so the existing path is unaffected.
- `rupu agent show` / `rupu workflow show` now print the raw file with highlighting (was a structured field-by-field render). `rupu agent list` still provides the table view.
- README modernized — current subcommand surface, accurate Slice status (A + B + C shipped; D + E deferred), new "Workflow triggers" subsection.
- `docs/providers.md` — Gemini API-key path marked shipped (was deferred).

### Internal
- New `EventConnector` trait in `rupu-scm` with GitHub + GitLab impls (Etag fast-path on GitHub; per-project label cache on GitLab).
- New `cmd/completers.rs` with cheap basename-walk over global + project agent/workflow dirs for tab completion.
- Workspace deps: `clap_complete = { version = "4", features = ["unstable-dynamic"] }`, `syntect = { version = "5", features = ["default-fancy"] }` (pure-Rust, no `onig` C dep).

## v0.4.9 — auto-resume after approval (2026-05-06)

### Fixed
- **Pressing `a` at an approval gate now resumes the run inline.** Previously
  the printer recorded the approval, printed a misleading "Run paused. Resume
  with: rupu workflow approve <run_id>" message, and detached. The user then
  had to invoke `rupu workflow approve` themselves to actually run the
  downstream step. Now `attach_and_print` returns an `AttachOutcome` enum;
  on `Approved` the CLI rebuilds `OrchestratorRunOpts` with `resume_from`
  set, spawns a fresh runner task, and re-attaches the same `LineStreamPrinter`
  in skip-header mode so the resumed steps slot right under the gate
  without re-printing the workflow header or prior step blocks.
- Misleading "Run paused" message replaced with the actual downstream step
  rendering live in the same terminal session.

### Added
- `AttachOutcome` enum (`Done` / `Detached` / `Approved { awaited_step_id }` /
  `Rejected`) and a new `attach_and_print_with(opts)` variant taking
  `AttachOpts { skip_header, skip_count }` for the resume re-attach.
- `rupu watch` now prints a clear "Step approved — run `rupu workflow
  approve <run_id>`" message when `a` is pressed in watch mode (the watcher
  process doesn't have the workflow YAML / factory needed to spin a resume
  itself).

## v0.4.8 — line-stream UI rebuild + Anthropic tool-name sanitizer (2026-05-06)

### Fixed
- **Anthropic API rejects MCP tool names with `.`** — same root cause as
  the OpenAI bug fixed in PR #60. Anthropic's `/v1/messages` validates
  custom tool names against `^[a-zA-Z0-9_-]{1,128}$`, which rejects all
  rupu MCP tools (`scm.repos.list`, `issues.create`, `github.workflows_dispatch`,
  etc.). Applied the same `__dot__` escape symmetrically: sanitize on
  outbound tool definitions and tool_use blocks in conversation history,
  desanitize on inbound tool_use events so the dispatcher receives the
  canonical name.
- **Spinner animation broke the line-stream output** — the previous
  `\x1b[s` / `\x1b[u` cursor-save spinner thread fought with the print
  thread for the cursor, causing assistant text and tool calls to land
  in the wrong place. Removed the animation entirely. The step header
  now prints a static `◐` glyph; the step footer (`✓` / `✗`) is the
  visual cue of completion. `SpinnerHandle` is kept as a no-op shim for
  caller compatibility.
- **Panel steps showed nothing in the UI** — panel steps emit no
  top-level transcript (each panelist has its own), so the prior printer
  rendered an empty block. The printer now detects panel steps via
  their `items[]` array, opens with a `◐ <step> (panel · N panelists)`
  header, and renders one child line per panelist
  (`├─ ✓ <agent>  · N findings`) with the aggregated tally in the
  footer.
- **`[v] view findings` was always empty** — the previous loader filtered
  for findings whose `step_id` matched the awaiting (gate) step, but
  findings live on the *upstream* panel step. Now aggregates from every
  prior step's `step_results.jsonl` entry that has findings.
- **`> ` prompt looked like a typed-input cue** — the user thought
  they had to type the letter and press Enter. Replaced with the
  affordance-explicit `[a/r/v/q]: ` marker.
- **`│` rail broke through blank lines** — assistant chunks containing
  empty lines used `chunk.lines()` which silently dropped them. Now
  uses `chunk.split('\n')` and emits a rail-only line for each empty
  segment so the visual column stays continuous.

### Added
- `LineStreamPrinter::panel_start` / `panel_done` / `panelist_line`
  for surfacing panel structure in the timeline.
- `print_findings(&[(step_id, Vec<FindingRecord>)])` now takes grouped
  input. Renders a `─── N findings ───` header, then per-finding
  badge + bold title + dim source line + wrapped body.
- Workspace-wide `sanitize_anthropic_tool_name` /
  `desanitize_anthropic_tool_name` helpers + 5 unit tests covering
  round-trip identity and message-history rewriting.

### Changed
- `phase_separator` no longer prints a leading rail line — `step_done`
  already emits one, so the doubled `│` was visual noise.

## v0.4.7 — clean missing-credential error + OpenAI models listing fix (2026-05-06)

Rolls forward v0.4.6 to include two fixes that landed after the v0.4.6 tag:

### Fixed
- **No more panic on missing credentials (PR #65)**: `StepFactory` previously
  `.expect()`-ed when no credential was present for the requested provider/mode,
  killing `rupu workflow run` with a backtrace. It now constructs a stub
  `LlmProvider` that returns a `ProviderError::AuthConfig` carrying the exact
  `rupu auth login --provider <p> --mode <m>` invocation to fix it. The error
  surfaces through the line-stream UI as a normal step failure with hint.
- **OpenAI `rupu models list` shows live models again (PR #66)**: the
  `chatgpt.com/backend-api/codex/models` response uses `slug` as the model
  identifier, not `id`. Replaced the strict `serde` deserializer with a lenient
  `extract_model_ids` walker that probes `id` / `slug` / `display_name` / `name`
  in order, and accepts arrays of strings or arrays of objects under
  `data` / `models` / a top-level array. Diagnostic logging now dumps top-level
  keys + first-entry keys + 2000-char preview when zero models parse, so
  future shape drift is self-debugging.

(All v0.4.6 changes below are also included.)

## v0.4.6 — interactive prompt + spinner + design polish (2026-05-06)

### Fixed
- **Single-key approval prompt (Bug 1)**: `approval_prompt` now uses
  `crossterm::terminal::enable_raw_mode()` + `crossterm::event::read()` so the
  user presses a single key (`a`, `r`, `v`, `q`) without needing Enter.
  Ctrl-C and Esc both map to `q` (detach). Raw mode is disabled immediately
  after the keypress so stdout continues streaming normally.
- **`[v] view findings` now works (Bug 2)**: pressing `v` at the approval gate
  reads the panel step's `FindingRecord`s from `step_results.jsonl` and
  pretty-prints them with severity-colored chip badges
  (`[ critical ]`, `[ high ]`, etc.) before re-showing the prompt. The loop
  repeats until the user presses `a`, `r`, or `q`.
- **Reject path prompts for reason**: pressing `r` now correctly prompts
  `"Reason (optional, Enter to skip):"` via line-buffered stdin (the one place
  where Enter is right), then calls `RunStore::reject`.

### Added
- **Animated spinner during streaming (Bug 3 + 4)**: `crates/rupu-cli/src/output/spinner.rs`
  — a new `Spinner` type that cycles `◐ ◓ ◑ ◒` every 125 ms via a background
  `std::thread`. `step_start` now saves the ANSI cursor position (`\x1b[s`)
  before the glyph and returns a `SpinnerHandle`; the spinner restores to that
  position on each tick so the glyph animates in-place while text streams
  below. Degrades to a no-op on non-TTY streams (pipes, CI runners).
- **Phase separator between workflow steps**: a dim `──────────────────────`
  line is inserted between each major step block for visual rhythm.
- `palette::write_bold_colored` — bold + colored variant for status glyphs and
  key identifiers.
- `BRAND_300` (#a78bfa) and `SEPARATOR` (#475569) palette entries.

### Changed
- **Hierarchy polish**: workflow name is now brand-500 bold in the header;
  step completion glyphs (`✓`, `✗`) and step IDs are bold+green/bold+red for
  clear terminal hierarchy.
- **Indent guides**: the `│` tree pipes are now brand-300 (#a78bfa) for a
  subtle warm purple thread through the run rather than plain gray.
- **Token + duration footer**: `✓ step  · 0.0s · 2997 tokens` — the `·`
  separator stays; tokens now shows as `N tokens` instead of `Nt` for clarity.
- **Workflow done/failed headers**: prepend a blank line before the footer so
  it's visually separated from the last step block.
- `crossterm` added to `rupu-cli` workspace dep declarations.

## v0.4.5 — line-stream output by default (canvas opt-in via --canvas) (2026-05-06)

### Also includes (PRs merged alongside)
- Stable credentials across signed-binary updates via file backend (PR #62 / #63)
- `rupu auth login` defaults to file backend; keychain becomes opt-in

### Changed
- `rupu run`, `rupu workflow run`, and `rupu watch` now default to a
  **streaming vertical timeline** printed line-by-line to stdout — no
  alt-screen takeover. Works in any terminal, pipe, or CI runner.
  Pass `--canvas` to get the full ratatui TUI.
- New `crates/rupu-cli/src/output/` module: `LineStreamPrinter` (Okesu
  palette, auto-degrades on `NO_COLOR`/pipes), `TranscriptTailer`
  (incremental JSONL byte-offset reader), and `workflow_printer`
  (polling loop that drives the printer from live or finished runs).
- `rupu watch` gains `--follow` (tail a live run) and `--replay` (pace
  through a finished run). Both default to line-stream; `--canvas`
  routes to rupu-tui for the alt-screen experience.
- Agent runner `suppress_stream_stdout` is now `true` for `rupu run`
  — the line-stream printer is the sole source of stdout, preventing
  duplicate output when piped.

## v0.4.4 — TUI canvas: stripe + rounded cards + connectors (2026-05-06)

### Fixed
- Tracing on stderr punched through the alt-screen and corrupted
  the canvas (`WARN`/`INFO` lines bleeding through, color reset
  state clobbered). TUI commands now route logs to
  `~/Library/Caches/rupu/rupu.log` (or `$XDG_CACHE_HOME/rupu/`);
  non-TUI commands keep stderr.
- LLM token-stream chunks were `print!()`ed to stdout from the
  agent runner even when the TUI owned the terminal — visible as
  the panelist JSON dump bleeding into the canvas. The workflow
  StepFactory now sets `suppress_stream_stdout: true`; the TUI
  reads tokens from the JSONL transcript instead. Single-agent
  `rupu run` keeps the stream (its TUI attach is deferred).
- Long `AssistantMessage` lines (panelist agents emit 2000+ char
  JSON) overflowed the focused-node panel as raw text. Per-line
  truncated at 80 chars with `…` indicator.
- Canvas drew bordered cards with no connector lines between
  them. Added `═══▶` for same-row hops, `║`/`╠`/`╚` for fan-out
  drops, all colored by upstream-node status.

### Changed
- Canvas redesigned to mirror the Okesu visual language: status
  stripe (`█` row colored by status) at the top of each card,
  rounded corners (`╭╮╰╯`), multi-row cards (5×22) showing
  step_id + glyph on row 1 and a status-derived secondary line
  (`done`, `running · agent`, `awaiting approval`, `done · 412t`)
  on row 2, dotted (`·`) backdrop. Bold-pulse on running/awaiting
  via wall-clock toggle.

## v0.4.3 — fix: OpenAI rejects tool names with `.` (2026-05-06)

### Fixed
- OpenAI's Responses API rejects tool names that don't match
  `^[a-zA-Z0-9_-]+$` with HTTP 400. The MCP catalog uses dotted names
  like `scm.repos.list_owned`; every workflow whose agent inherits the
  default tool set + uses `provider: openai` was unrunnable. Now
  encodes `.` as `__dot__` on send, decodes on receive — round-trip
  invisible to the rest of the agent runtime, which keeps using
  canonical (dotted) names.

## v0.4.2 — fix: github/gitlab credentials now actually readable (2026-05-06)

### Fixed
- `rupu repos list` could not see github/gitlab credentials stored by
  v0.4.1's `rupu auth login` because `KeychainResolver`'s internal
  provider whitelist (separate from the CLI's clap whitelist) didn't
  know about github/gitlab. The resolver now recognizes both.
- `rupu auth status` showed `-` for the github SSO column even after a
  successful login, because GitHub device-code grants don't carry an
  `expires_at` and `peek_sso` was returning `None`. Now renders
  `✓ (no expiry)`.

## v0.4.1 — github / gitlab in `rupu auth login` (2026-05-05)

### Added
- `rupu auth login --provider github` and `--provider gitlab` are now accepted
  (the library plumbing already shipped in Slice B-2; only the CLI whitelist
  was gating it). Both providers also appear in `rupu auth status` and are
  cleared by `rupu auth logout --all`.

  Login modes:
  - `--mode sso` — device-code OAuth (GitHub) or PKCE callback (GitLab).
    GitLab SSO needs a real OAuth client_id; PAT path works today.
  - `--mode api-key` — personal access token (read from stdin or `--key`).

  Unblocks `rupu repos list` for both platforms.

## v0.4.0 — Slice C: TUI (2026-05-05)

### Added
- New `rupu-tui` crate: live + replay terminal viewer for runs.
- New `rupu watch <run_id>` subcommand (eleventh top-level command).
  - `--replay [--pace=N]` to replay a finished run.
- Canvas view (Okesu-mirror, horizontal LTR) + Tree view (vertical TTB),
  toggle with `v`. Default depends on terminal width.
- Inline approval flow: focus the `⏸` node and press `a` to approve or
  `r` to reject with a reason. Same `RunStore` library functions the CLI
  uses, no race with `rupu workflow approve` from another shell.
- Status glyph palette: `●  ◐  ✓  ✗  !  ○  ↺  ⏸  ⊘` with status-colored
  edges.
- `NO_COLOR=1` and `RUPU_TUI_DEFAULT_VIEW=tree|canvas` env-var support.

### Changed
- `rupu workflow run` opens the TUI by default. To get the old
  text-progress output, use the `--no-attach` flag (if implemented) or
  read the JSONL transcript directly.

### Deferred
- Single-agent `rupu run` TUI attach (per-run dir layout mismatch).

### Internal
- `RunStore::approve` / `RunStore::reject` library functions factored
  out of `cli::cmd::workflow` (used by both the CLI text wrappers and
  the TUI inline approve/reject).

## v0.3.0 — Slice B-3: `rupu init` (2026-05-04)

### Added

- **`rupu init [PATH] [--with-samples] [--force] [--git]`** bootstraps a
  project's `.rupu/` directory in one command.
- **Curated template set** (`--with-samples`): 6 agent templates
  (`review-diff`, `add-tests`, `fix-bug`, `scaffold`, `summarize-diff`,
  `scm-pr-review`) plus one workflow (`investigate-then-fix`), embedded
  via `include_str!` so `cargo install` users don't need network on
  first run.
- **`.gitignore`** auto-managed: `.rupu/transcripts/` is appended on
  init (idempotent).
- **`--git`** flag runs `git init` if the target isn't already in a
  repo. Missing `git` on PATH is a warning, not a hard error.

### Internal

- New module `crates/rupu-cli/src/templates.rs` with the manifest;
  bidirectional sync test ensures `templates/` and the manifest
  never drift apart.
- `rupu-cli` subcommand count: 9 → 10.

## v0.2.0 — Slice B-2: SCM + issue trackers (2026-05-04)

### Added

- **GitHub + GitLab connectors** (`rupu-scm`). RepoConnector + IssueConnector
  trait families with per-platform impls; ETag-cached + retry-with-backoff
  + per-platform Semaphore. classify_scm_error pure function gives every
  error a recoverable/unrecoverable verdict.
- **Embedded MCP server** (`rupu-mcp`). 17 tools (`scm.repos.*`, `scm.prs.*`,
  `scm.files.read`, `scm.branches.*`, `issues.*`, `github.workflows_dispatch`,
  `gitlab.pipeline_trigger`) auto-attached to every `rupu run` /
  `rupu workflow run`. JSON-Schema-typed via schemars; permission gating
  honors the agent's frontmatter `tools:` list AND `--mode` flag.
- **`rupu auth login --provider github|gitlab`** with both api-key and SSO
  flows. SSO uses GitHub's device-code flow / GitLab's browser-callback PKCE
  flow.
- **`rupu repos list [--platform <name>]`** — table-rendered list of repos
  the user can access on configured platforms.
- **`rupu mcp serve [--transport stdio]`** — MCP server for external
  clients (Claude Desktop, Cursor, etc.).
- **`rupu run <agent> [<target>]`** — optional positional arg; `target` is
  a `<platform>:<owner>/<repo>[#N | !N | /issues/N]` reference. The runner
  clones the repo to a tmpdir (or reuses the cwd) and preloads a
  `## Run target` section into the agent's system prompt.

### Architecture

- Two new crates: `rupu-scm` (connectors) and `rupu-mcp` (MCP kernel).
- `rupu-agent`'s `run_agent` now spins up `rupu_mcp::serve_in_process`
  before the first turn and tears it down before returning. SCM tools
  appear alongside the six built-in tools through a thin `McpToolAdapter`.
- `Registry::discover` builds connectors from the same `KeychainResolver`
  + `Config` that LLM-provider auth uses; missing credentials skip the
  platform silently with an INFO log.

### Internal

- `Platform` and `IssueTracker` enums in `rupu-scm` cover GitHub + GitLab
  today; `IssueTracker::Linear` and `IssueTracker::Jira` exist so future
  adapters slot in without reshaping call sites.
- New workspace deps: `octocrab`, `gitlab`, `git2` (vendored libgit2 +
  vendored OpenSSL), `lru`, `schemars`, `comfy-table`, `jsonschema`.

### Docs

- `docs/scm.md` — canonical reference (capabilities, auth, target syntax,
  full tool catalog, config schema, error classification, troubleshooting).
- `docs/scm/github.md` + `docs/scm/gitlab.md` — per-platform walkthroughs.
- `docs/mcp.md` — Claude Desktop / Cursor wiring + sample config.
- README + CHANGELOG updates.

## v0.1.5 — Anthropic SSO mirrors pi-mono (2026-05-03)

### Fixed

- **Anthropic SSO** finally aligned against pi-mono's `anthropic.ts` (a known-working third-party reference at `github.com/badlogic/pi-mono/packages/ai/src/utils/oauth/anthropic.ts`). Five differences vs `v0.1.4`:
  - **Authorize URL**: back to `https://claude.ai/oauth/authorize` (v0.1.3's `claude.com/cai/oauth/authorize` was a misread; pi-mono hits `claude.ai` directly).
  - **Scopes**: added `org:create_api_key` and `user:file_upload` to match pi-mono's full set.
  - **State**: now equals the PKCE verifier (Anthropic-specific). pi-mono's impl does this; we were using a fresh random nonce.
  - **Token-exchange body**: JSON (`Content-Type: application/json`), not form-encoded. The token endpoint apparently rejects form-encoded bodies for this client.
  - **Token-exchange body**: now includes `state`. OAuth standard doesn't require it at exchange time but Anthropic's server expects it.

### Internal

- `ProviderOAuth` gains three per-provider fields: `token_body_format` (`Form` | `Json`), `state_is_verifier` (Anthropic quirk), `include_state_in_token_body` (Anthropic quirk). OpenAI / Gemini / Copilot keep the standard form-encoded, random-state, no-state-in-body shape.

## v0.1.4 — Anthropic SSO request format (2026-05-03)

### Fixed

- **Anthropic SSO** "Invalid request format" error finally resolved by extracting Claude Code's actual URL builder (`GI_` function) from the binary. Two missing pieces:
  - The request must include `code=true` as a query parameter (Claude Code appends it as the FIRST param). Omitting it is what claude.ai's authorize endpoint rejects as "Invalid request format". This wasn't documented anywhere; only visible by reading the prod URL builder.
  - The `redirect_uri` must use literal `localhost`, not `127.0.0.1`. Claude Code hardcodes `http://localhost:${port}/callback`.

The decoded URL builder (the function rupu must mirror):

```js
function GI_({ codeChallenge, state, port, ... }) {
  let url = new URL(O ? CLAUDE_AI_AUTHORIZE_URL : CONSOLE_AUTHORIZE_URL);
  url.searchParams.append("code", "true");                                  // ← was missing
  url.searchParams.append("client_id", CLIENT_ID);
  url.searchParams.append("response_type", "code");
  url.searchParams.append("redirect_uri", `http://localhost:${port}/callback`);
  // ... scope, code_challenge, code_challenge_method, state
}
```

Two new regression tests pin both behaviors so the next dive into Claude Code's binary doesn't have to re-discover them.

## v0.1.3 — Anthropic SSO regression fix (2026-05-03)

### Fixed

- **Anthropic SSO** — `v0.1.2` was a regression. Authorized URL switched to `platform.claude.com/oauth/authorize` (the **Console** flow, for API-customer organizations issuing console-managed API keys), not the SSO flow that paid Claude.ai subscribers actually use. Verified by extracting the prod config object literal from Claude Code's binary at `/Users/matt/.local/share/claude/versions/2.1.126`:

```js
{
  CONSOLE_AUTHORIZE_URL:  "https://platform.claude.com/oauth/authorize",   // wrong path for SSO
  CLAUDE_AI_AUTHORIZE_URL: "https://claude.com/cai/oauth/authorize",       // ← SSO
  TOKEN_URL:               "https://platform.claude.com/v1/oauth/token",
  CLIENT_ID:               "9d1c250a-e61b-44d9-88ed-5944d1962f5e",
}
```

  - Authorize URL: now `https://claude.com/cai/oauth/authorize`.
  - Client ID: reverted to the UUID `9d1c250a-...` (the literal `CLIENT_ID` from the prod config; the metadata URL that v0.1.2 used is a separate registration document, not the OAuth client_id).
  - Token URL stays at `platform.claude.com/v1/oauth/token` (correct in v0.1.2).
  - Regression test pinned to lock the SSO-not-Console choice.

## v0.1.2 — Anthropic SSO follow-up hotfix (2026-05-03)

### Fixed

- **Anthropic SSO** — `v0.1.1`'s scope-set fix wasn't enough. The actual root cause was that the entire OAuth client identity was wrong:
  - **`client_id`** must be `https://claude.ai/oauth/claude-code-client-metadata` (a URL, per RFC 7591 dynamic client registration), not the stale UUID `9d1c250a-...` we had baked in.
  - **`authorize_url`** moved from `claude.ai/oauth/authorize` (returns 403) to `platform.claude.com/oauth/authorize` (returns 200).
  - **`token_url`** moved from `console.anthropic.com/v1/oauth/token` to `platform.claude.com/v1/oauth/token`.
  
  Verified by fetching the published DCR metadata and confirming the endpoint behaviors. This is the OAuth identity Claude Code actually uses (`client_name: "Claude Code"` in the metadata document); we continue to impersonate it pending rupu-specific OAuth client registration.

## v0.1.1 — SSO hotfix (2026-05-03)

### Fixed

- **Anthropic SSO** now succeeds against `claude.ai/oauth/authorize`. The previous scope set mixed Console-flow scopes (`org:create_api_key`) into the claude.ai authorize call, which `claude.ai` rejected with "Invalid request format". The new scope set is the full Claude Code request shape (`user:inference`, `user:profile`, `user:sessions:claude_code`, `user:mcp_servers`) — matches what users see on the consent screen since we use Claude Code's OAuth client_id, and avoids re-login when we eventually surface session/MCP features.
- **OpenAI SSO** now matches the Codex CLI request shape verified against `openai/codex codex-rs/login/src/server.rs`:
  - `token_url` corrected from `console.anthropic.com/v1/oauth/token` (a copy-paste bug from Plan 2 Task 4) to `auth.openai.com/oauth/token`.
  - Redirect URI uses fixed ports `1455` (with `1457` fallback) on `localhost`, path `/auth/callback` — these are pinned by OpenAI's Hydra registration for the `app_EMoamEEZ73f0CkXaXp7hrann` client.
  - Scopes extended with `api.connectors.read api.connectors.invoke`.
  - Authorize URL now sends the Codex CLI extras: `id_token_add_organizations=true`, `codex_cli_simplified_flow=true`, `originator=codex_cli_rs`.

### Internal

- `ProviderOAuth` (`crates/rupu-auth/src/oauth/providers.rs`) gains three new fields — `redirect_host`, `fixed_ports`, and `extra_authorize_params` — so each provider can declare its specific redirect-URI shape and additional authorize-query parameters without per-provider branching in the callback flow.
- The redirect listener (`oauth/callback.rs`) walks `fixed_ports` in order before falling back; `None` keeps the original OS-assigned port-0 behavior.

### Honest acknowledgements

We currently impersonate Claude Code's and Codex CLI's OAuth clients. The consent screen reads "Claude Code wants access ..." and "Codex CLI wants access ..." rather than "rupu wants ...". This is necessary while we use their pre-registered redirect URIs and scope sets; the long-term fix (registering rupu-specific OAuth clients with each vendor) is tracked in `TODO.md`.

## v0.1.0 — Slice B-1: Multi-provider wiring (2026-05-02)

### Added

- **OpenAI, Gemini, GitHub Copilot provider adapters wired end-to-end.** Anthropic remains the most exercised; Gemini API-key path via AI Studio is deferred to a follow-up (see `TODO.md`).
- **SSO authentication for all four providers:**
  - Browser-callback (PKCE) for Anthropic, OpenAI, Gemini.
  - GitHub device-code for Copilot (mirrors `gh auth login` UX).
- **`CredentialResolver` trait + `KeychainResolver` impl** with refresh-on-expiry. Per-credential keychain entries (`rupu/<provider>/<api-key|sso>`).
- **Default auth precedence:** SSO wins when both modes configured. Override by setting `auth: api-key` or `auth: sso` in agent frontmatter.
- **`rupu auth login --mode <api-key|sso>`.**
- **`rupu auth logout --provider X [--mode <m>]`** and **`rupu auth logout --all [--yes]`**.
- **`rupu auth status`** two-column rendering: `PROVIDER  API-KEY  SSO  (expires in Yd)`.
- **`rupu models list [--provider X]`** — custom + live-fetched + baked-in entries with source labels.
- **`rupu models refresh [--provider X]`** — re-fetch `/models` for each configured provider; cache at `~/.rupu/cache/models/<provider>.json` (TTL 1h).
- **`[providers.<name>]` config block** in `~/.rupu/config.toml`: `base_url`, `org_id`, `region`, `timeout_ms`, `max_retries`, `max_concurrency`, `default_model`, `[[providers.X.models]]`.
- **`Event::Usage { provider, model, input_tokens, output_tokens, cached_tokens }`** written to JSONL transcripts per response.
- **Anthropic prompt-cache** integration: `cache_read_input_tokens` decoded into `Usage.cached_tokens`.
- **`rupu run` header** (`agent: X  provider: Y/Z  model: M`) and **footer** (`Total: I input / O output tokens`).
- **`--no-stream`** flag on `rupu run` (default is streaming with on-the-fly TextDelta print to stdout).
- **Documentation:** `docs/providers.md` canonical reference + four `docs/providers/<name>.md` per-provider walkthroughs.
- **Nightly live-integration test workflow** gated by `RUPU_LIVE_TESTS=1`. Anthropic / OpenAI / Copilot covered; Gemini deferred.
- **Per-provider concurrency semaphore** (`Anthropic 4, OpenAI 8, Gemini 4, Copilot 4` defaults; configurable). Rate-limit isolation across vendors.
- **Per-vendor `classify_error()`** pure functions mapping HTTP status + body + vendor code → structured `ProviderError` variants (`RateLimited`, `Unauthorized`, `QuotaExceeded`, `ModelUnavailable`, `BadRequest`, `Transient`, `Other`).

### Changed

- **`AgentSpec` frontmatter** now accepts optional `auth: <api-key|sso>` field.
- **`provider_factory`** consults `CredentialResolver` instead of `AuthBackend` directly. Slice A's env-var fallback (`ANTHROPIC_API_KEY` etc.) is dropped at this layer; explicit `rupu auth login` is the documented path. The nightly live-test suite re-introduces env-var support behind `RUPU_LIVE_TESTS` for CI only.
- **Sample agents** in `.rupu/agents/` updated to demonstrate `auth:` (`sample-openai.md`, `sample-gemini.md`, `sample-copilot.md`, `sample-anthropic-sso.md`).

### Backward-compatible

- **Existing Slice A agent files** (`provider: anthropic` only) load unchanged. Missing `auth:` triggers the default-precedence path.
- **Legacy keychain entries** (Slice A's `rupu/<provider>` shape) are still readable by the resolver as API-key on first lookup.

### Deferred (see `TODO.md`)

- macOS keychain code-signing + notarization (highest-impact UX bug; track via TODO.md).
- `rupu usage` aggregation subcommand (Slice D).
- Gemini API-key path via AI Studio.
- Copilot `default_model` literal alignment.
- `classify::truncate` UTF-8 walk-back regression test gap.

## v0.0.3-cli — Slice A (2026-04-XX)

Initial single-binary release: Anthropic provider, agent file format, JSONL transcripts, action protocol, permission resolver, linear workflow runner, OS keychain auth backend.
