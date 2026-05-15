# rupu

**Status:** local CLI feature-complete (Slices A + B + C shipped)

---

## What is rupu?

`rupu` is a CLI for orchestrating coding agents across repositories — driven by
issue-tracker events, gated by human approvals when you want them, with a JSONL
transcript on every run. A single Rust binary that:

- Drives any of four LLM providers (Anthropic, OpenAI, Gemini, GitHub Copilot)
  via API key OR SSO, with credentials kept in the OS keychain or a chmod-600 file.
- Loads agent + workflow definitions from `.rupu/` in your project (or globally
  from `~/.rupu/`); ships a curated starter set via `rupu init --with-samples`.
- Talks to GitHub and GitLab through a single embedded MCP server (so the same
  surface works inside rupu and inside Claude Desktop / Cursor / any MCP host).
- Fires workflows on cron schedules OR external SCM events (GitHub / GitLab),
  via either a system-cron poll loop (no daemon) or a user-managed
  `rupu webhook serve` long-running process.
- Renders runs as live terminal streams (`rupu workflow run` / `rupu run`),
  with `rupu watch <run_id>` to re-attach to anything in flight.

What's NOT in this binary yet: the SaaS dashboard, the remote sandbox runtime,
and the native desktop app — those are slices D + E. See [TODO.md](TODO.md) for
deferred items in already-shipped slices.

---

## Install

**From source (requires Rust 1.77+):**

```sh
cargo install --git https://github.com/Section9Labs/rupu
```

**Prebuilt binary:**

Download the tarball for your platform from the
[Releases](https://github.com/Section9Labs/rupu/releases) page and place `rupu`
somewhere on your `$PATH`.

---

## Quick start

```bash
# 1. Bootstrap a new project
rupu init --with-samples --git

# 2. Authenticate at least one provider
rupu auth login --provider anthropic --mode sso

# 3. Run an agent
rupu run review-diff
```

`rupu init --with-samples` seeds the focused single-agent helpers
(`review-diff`, `add-tests`, `fix-bug`, `scaffold`, `summarize-diff`,
`scm-pr-review`) plus a fuller project-oriented sample library for
issue intake, spec writing, phase planning, PR review panels, phased
delivery, contract schemas, and autonomous controller samples under
`.rupu/`. Re-running is a no-op; pass `--force`
to overwrite local template customizations with the latest embedded
versions.

---

`rupu watch <run_id>` re-attaches to any historic run. Add `--replay
--pace=20` to replay a finished run for review.

---

### Authenticate

rupu supports four providers; each works with API-key auth or SSO.

| Provider  | API key                              | SSO                                |
| --------- | ------------------------------------ | ---------------------------------- |
| anthropic | `console.anthropic.com` → API Keys   | Claude.ai login (browser callback) |
| openai    | `platform.openai.com` → API Keys     | ChatGPT login (browser callback)   |
| gemini    | `aistudio.google.com` → Get API Key  | Google account (browser callback)  |
| copilot   | (PAT via `gh` token)                 | GitHub login (device code)         |

```sh
# API key
rupu auth login --provider anthropic --mode api-key --key sk-ant-XXX

# SSO (opens a browser; Copilot prints a device code instead)
rupu auth login --provider anthropic --mode sso

# Verify
rupu auth status
```

Credentials are stored at `~/.rupu/auth.json` (chmod-600 file, the default —
matches `gh`, `aws`, `gcloud`). To use the OS keychain instead:
`rupu auth backend --use keychain`. SSO entries auto-refresh near expiry;
failure surfaces an actionable error pointing at `rupu auth login --mode sso`.

See `docs/providers.md` for the full reference and `docs/providers/<name>.md`
for per-provider walkthroughs.

## SCM & issue trackers

rupu integrates with GitHub and GitLab through a single embedded MCP
server. Agents call typed tools (`scm.prs.diff`, `issues.get`, ...) and
the right per-platform connector dispatches the call. See `docs/scm.md`
for the full reference.

```bash
# 1. Authenticate
rupu auth login --provider github --mode sso

# 2. List your repos
rupu repos list

# 3. Run an agent against a PR
rupu run review-pr github:section9labs/rupu#42

# 4. Or expose the same surface to Claude Desktop / Cursor:
rupu mcp serve --transport stdio
```

| Capability             | GitHub | GitLab |
|------------------------|:------:|:------:|
| Repos / branches       |   ✅   |   ✅   |
| PRs / MRs              |   ✅   |   ✅   |
| Issues                 |   ✅   |   ✅   |
| Workflows / pipelines  |   ✅   |   ✅   |
| Clone to local         |   ✅   |   ✅   |
| Polled event triggers  |   ✅   |   ✅   |
| Webhook event triggers |   ✅   |   ✅   |

Linear and Jira now ship as native trigger sources:

- webhook ingress for normalized tracker state events
- polling via `poll_sources = ["linear:<team-id>"]`
- polling via `poll_sources = ["jira:<site>/<project>"]` or `["jira:<project>"]` with `[scm.jira].base_url`
- tracker-native autoflow ownership for repo-bound autonomous execution

They are not full repo / PR backends.

### Workflow triggers

A workflow can fire on a cron schedule or in response to an SCM event
(issue opened, PR merged, issue labeled, …). Three runtime tiers — pick
whichever matches your environment:

| Tier | When it fires | Where it lives |
|---|---|---|
| Cron polling | system cron / launchd → `rupu cron tick` | every install (no daemon) |
| Webhook serve | inbound HTTP from GitHub / GitLab | user-managed long-running process |
| Cloud relay | rupu.cloud receives webhooks, CLI consumes | Slice E (future) |

```yaml
# .rupu/workflows/triage-on-label.yaml
name: triage-on-label
trigger:
  on: event
  event: github.issue.labeled
  filter: "{{ event.payload.label.name == 'triage' }}"
steps:
  - id: classify
    agent: triage-classifier
    prompt: "Classify {{ event.repo.full_name }}#{{ event.payload.issue.number }}"
```

See [`docs/triggers.md`](docs/triggers.md) for the full vocabulary, glob-pattern
matching (`github.issue.*`), and label-as-queue patterns.

### Run your first agent

The rupu repository ships sample agents in `.rupu/agents/`. If you run `rupu` from
inside the rupu checkout, project-discovery picks them up automatically — the same
mechanism end-users use in their own repos.

```sh
cd /path/to/rupu
rupu run fix-bug "make the failing test pass"
```

A JSONL transcript is written to `~/.rupu/transcripts/<run-id>.jsonl`.

### Use the samples in your own project

```sh
cd ~/projects/your-repo
rupu init --with-samples --git
rupu run review-diff "look for bugs and missing tests"
rupu run summarize-diff "summarize changes since main"
```

---

## Where things live

### Global (`~/.rupu/`)

| Path | Purpose |
|------|---------|
| `~/.rupu/config.toml` | Global config (default provider, log level, …) |
| `~/.rupu/auth.json` | Stored provider credentials |
| `~/.rupu/repos/` | Repo-to-local-checkout bindings for autonomous runs |
| `~/.rupu/autoflows/` | Persistent issue claims and worktree state |
| `~/.rupu/contracts/` | Global reusable contract schemas |
| `~/.rupu/transcripts/` | JSONL run transcripts |
| `~/.rupu/cache/` | Scratch space + crash logs |
| `~/.rupu/workspaces/` | Reserved for Slice C session state |

### Per-project (`<project>/.rupu/`)

| Path | Purpose |
|------|---------|
| `<project>/.rupu/agents/` | Agent `.md` files for this repo |
| `<project>/.rupu/contracts/` | Repo-local JSON Schemas for workflow handoffs |
| `<project>/.rupu/workflows/` | Workflow YAML files for this repo |
| `<project>/.rupu/config.toml` | Project-local config overrides |

---

## Example runs

### Summarise what changed

```sh
rupu run summarize-diff "what changed in the last three commits?"
```

The `summarize-diff` agent reads `git diff` output and returns a commit-message-style
summary. Useful before writing a PR description.

### Review a diff for issues

```sh
rupu run review-diff "check staged changes for bugs and missing tests"
```

`review-diff` inspects staged (or HEAD) diff and reports bugs, code smells, and
coverage gaps.

---

## Documentation

- `docs/using-rupu.md` — practical day-to-day usage
- `docs/agent-format.md` — complete agent schema reference
- `docs/agent-authoring.md` — how to write good agents
- `docs/workflow-format.md` — complete workflow schema reference
- `docs/workflow-authoring.md` — how to design good workflows
- `docs/development-flows.md` — recommended engineering flows
- `examples/README.md` — copyable agents and workflows

## Subcommands

```
rupu init [--with-samples] [--git]    Bootstrap .rupu/ in the current dir
rupu run <agent> [prompt]             Run an agent from the project's .rupu/agents/
rupu agent {list, show, edit}         Manage agents (list / inspect / open in $EDITOR)
rupu workflow {list, show, edit}      Manage workflows
rupu workflow run <name> [target]     Run a workflow (target: repo, PR, or issue ref)
rupu workflow runs                    List recent persisted runs
rupu workflow {approve, reject} <id>  Resume / cancel a paused-for-approval run
rupu watch <run_id> [--replay]        Re-attach to any past or in-flight run
rupu transcript {list, show}          Browse JSONL transcripts
rupu issues {list, show, run}         Issue-tracker surface (auto-detects from cwd)
rupu repos list                       List configured-platform repositories
rupu cron {list, tick, events}        Cron + polled-event trigger runtime
rupu webhook serve [--addr]           Long-lived webhook receiver for GitHub / GitLab / Linear / Jira
rupu mcp serve [--transport]          Expose rupu's tools to MCP clients
rupu auth {login, logout, status}     Provider credential management
rupu models {list, refresh}           Browse / refresh discovered model lists
rupu ui {themes, theme ...}           List, inspect, validate, and import UI themes
rupu config {get, set}                Read / write rupu configuration
rupu completions {print, install}     Shell-completion scripts (with dynamic agent names)
rupu usage                            Usage reports across transcripts + workflow runs
```

Run `rupu <subcommand> --help` for the full surface of any one. Tab completion
covers every flag and dynamically lists agent / workflow names plus session /
transcript ids for the relevant positional slots once shell integration is
installed.

Structured output is standardized as:

- `table` — default human view
- `json` — structured detail or collection report
- `csv` — collection/report views with stable row shapes

Collection views that support `--format json|csv` include:

- `rupu agent list`
- `rupu auth status`
- `rupu cron list`
- `rupu cron events`
- `rupu issues list`
- `rupu models list`
- `rupu repos list`
- `rupu repos tracked`
- `rupu session list`
- `rupu transcript list`
- `rupu usage`
- `rupu workflow list`
- `rupu workflow runs`
- `rupu autoflow list`
- `rupu autoflow wakes`
- `rupu autoflow status`
- `rupu autoflow claims`
- `rupu autoflow history`
- `rupu autoflow doctor`
- `rupu ui themes`

Detail views that support `--format json` include:

- `rupu agent show`
- `rupu auth backend`
- `rupu issues show`
- `rupu session show`
- `rupu workflow show`
- `rupu workflow show-run`
- `rupu transcript show`
- `rupu autoflow show`
- `rupu autoflow explain`
- `rupu ui theme show`
- `rupu ui theme validate`
- `rupu ui theme import`

Event/timeline views use a separate contract:

- `rupu transcript show`
  - `pretty` (default retained transcript snapshot)
  - `json`
  - `jsonl`
- `rupu workflow show-run`
  - `pretty` (default retained workflow snapshot)
  - `json`

Snapshot views with a custom structured surface:

- `rupu issues show` (`table` and `json`)
- `rupu workflow show-run` (`pretty`/`table` and `json`)
- `rupu autoflow monitor` (`table` and `json`)

UI theming is split into two layers:

- syntax highlighting theme (`[ui.syntax].theme`)
- CLI palette theme (`[ui.palette].theme`)

The simple path is a single shared selector:

```toml
[ui]
theme = "catppuccin-mocha"
live_view = "focused"
```

That applies the same named theme across syntax and palette when both exist. If the
shared name only matches one side directly, `rupu` falls back to the palette theme's
syntax hint.

Example:

```toml
[ui.syntax]
theme = "Solarized (dark)"

[ui.palette]
theme = "tokyo-night"
```

Use `rupu ui themes` to list built-in and installed themes, and `rupu ui theme import`
to install a local or remote Base16/native theme file into `~/.rupu/themes/` or
`<repo>/.rupu/themes/`.

Interactive/event-driven surfaces also support a shared live view mode:

- `[ui].live_view = "focused" | "compact" | "full"`
- per-command override: `--view focused|compact|full`

Mode semantics:

- `focused` — graph/timeline first, semantic summaries only
- `compact` — same timeline structure, full assistant messages, trimmed tool payload previews
- `full` — same timeline structure, full assistant/tool payload bodies with highlighting
- `rupu autoflow serve` uses the same modes inside an operator-console layout: issue list on one side, selected issue detail/timeline on the other when the terminal is wide enough
- `rupu autoflow monitor --watch` now uses the same operator-console layout in a read-only mode
- `rupu autoflow monitor` in a terminal now renders the same issue-list + selected-issue snapshot instead of the old summary frame
- `rupu autoflow history --watch` now uses the same retained operator-console layout, but with history rows as the selected issue timeline
- `rupu autoflow history` in a terminal now renders the same issue-list + selected-issue snapshot instead of the old flat history table

The first commands wired to this are:

- `rupu run`
- `rupu transcript show`
- `rupu session show`
- `rupu session attach`
- `rupu workflow run`
- `rupu watch`
- `rupu autoflow serve`

`rupu transcript show` now uses the same retained static-snapshot model as the live views. The
human `pretty` surface respects the configured theme, `--view focused|compact|full`, and optional
`--no-color`, `--pager`, and `--no-pager` overrides while keeping `json` and `jsonl` unchanged for
automation.

`rupu session show` now uses the same retained static-snapshot model for human terminal output.
The default table/human surface respects `--view focused|compact|full` plus `--no-color`, `--pager`,
and `--no-pager`. `full` expands the retained transcript content inline under each recent run,
while `--format json` remains unchanged for automation.

`rupu issues show` now uses the same retained static-snapshot model for human terminal output.
The default table/human surface keeps issue metadata and the highlighted body together on one
screen. Use `--no-color`, `--pager`, or `--no-pager` to override the configured UI preferences
for a single invocation, while `--format json` remains unchanged for automation.

Built-in parity names currently include:

- `catppuccin-mocha`
- `tokyo-night`
- `dracula`
- `gruvbox-dark`
- `github-dark`
- `github-light`
- `solarized-dark`
- `solarized-light`

If older standalone `rupu run` transcripts predate usage sidecars, repair them with
`rupu usage backfill`.

---

## Architecture overview

See [`docs/spec.md`](docs/spec.md) for the full architecture. Short version:

- **Agents** are `.md` files with YAML frontmatter for provider, model, tools,
  permission mode, and optional reasoning / output controls, plus a markdown system
  prompt body.
- **Workflows** are YAML orchestration files with sequential steps plus `for_each`,
  `parallel`, `panel`, `approval`, and trigger support.
- **Transcripts** are append-only JSONL files, and workflow runs are also tracked in
  the persistent run store for re-attach, approval, and history.
- **Sessions** are persistent agent containers that own multiple standalone runs over
  time; use `rupu session start`, `rupu session send`, and `rupu session attach` for
  long-lived agent conversations. Sessions can also be archived, restored, listed with
  `--all|--archived`, pruned by age, permanently deleted with `--force`, or cleaned in
  bulk with `rupu cleanup`.
- **Tool policy** lives in each agent's `tools:` and `permissionMode`; workflow
  `actions:` is a separate action-protocol allowlist, not a tool allowlist.

---

## Agents are code

Bypass mode runs arbitrary shell commands on your machine. Review every agent file
before you run it, just as you would review a shell script. An agent's `tools:` list
and `permissionMode` define its tool surface; workflow `actions:` is a separate
mechanism and does not replace tool policy. Treat an agent you did not write with
the same caution you would treat untrusted code.

---

## Hacking / development

```sh
git clone https://github.com/Section9Labs/rupu
cd rupu
cargo build --workspace
cargo test --workspace
```

MSRV: **1.77**. Set `RUPU_LOG=debug` for verbose tracing output.

---

## License

[MIT](LICENSE)
