# rupu

**Status:** Slice A — local CLI

---

## What is rupu?

`rupu` is "Okesu, but for code development in an agentic engineering world": a CLI for
orchestrating coding agents across repositories, triggered by issues from any tracker,
with sandboxed sessions that can be saved and resumed. The full vision spans SaaS,
native desktop, and multi-SCM connectors. **Slice A** — what you have now — is the
local-only foundation: a single Rust binary that loads agent definitions from your
project, drives a real LLM provider, and writes a JSONL transcript on every run.
Slice B adds SCM connectors and issue-tracker triggers; Slice C adds the control plane,
remote sandboxes, and session persistence.

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

`rupu init --with-samples` seeds six curated agent templates
(`review-diff`, `add-tests`, `fix-bug`, `scaffold`, `summarize-diff`,
`scm-pr-review`) plus one workflow (`investigate-then-fix`) under
`.rupu/`. Re-running is a no-op; pass `--force` to overwrite local
template customizations with the latest embedded versions.

---

## TUI

`rupu workflow run` opens a live terminal canvas of the in-flight run.
See `docs/tui.md` for full key bindings and surfaces.

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

Credentials are stored in the OS keychain at `rupu/<provider>/<api-key|sso>`.
SSO entries auto-refresh near expiry; failure surfaces an actionable error
pointing at `rupu auth login --mode sso`.

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

Linear and Jira issue trackers are designed-in but not shipped in this
release; see [TODO.md](TODO.md) for the deferred-feature list.

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
| `~/.rupu/transcripts/` | JSONL run transcripts |
| `~/.rupu/cache/` | Scratch space + crash logs |
| `~/.rupu/workspaces/` | Reserved for Slice C session state |

### Per-project (`<project>/.rupu/`)

| Path | Purpose |
|------|---------|
| `<project>/.rupu/agents/` | Agent `.md` files for this repo |
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

## Subcommands

```
rupu run <agent> [prompt]          Run an agent from the project's .rupu/agents/
rupu agent list                    List available agents
rupu agent show <name>             Print agent definition
rupu workflow list                 List available workflows
rupu workflow show <name>          Print workflow definition
rupu workflow run <name> [prompt]  Run a multi-step workflow
rupu transcript list               List past run transcripts
rupu transcript show <id>          Stream a transcript to stdout
rupu config get [key]              Read global config
rupu config set <key> <value>      Write global config
rupu auth login --provider <p>     Store a provider credential
rupu auth logout --provider <p>    Remove a provider credential
rupu auth status                   Show stored credentials
```

---

## Architecture overview

See [`docs/spec.md`](docs/spec.md) for the full architecture. Short version:

- **Agents** are `.md` files with YAML frontmatter (name, provider, model, tools,
  permissions) and a markdown system prompt as the body.
- **Workflows** are YAML files with a linear sequence of steps; each step names an
  agent and optionally passes context from previous steps.
- **Transcripts** are append-only JSONL files — one event per line. Every run writes
  one regardless of whether it succeeds or fails.
- **Action protocol** gates what tools an agent may call. The runtime validates every
  proposed action against an allowlist before execution.

---

## Agents are code

Bypass mode runs arbitrary shell commands on your machine. Review every agent file
before you run it, just as you would review a shell script. The action-protocol
allowlist in the agent frontmatter controls what tools the runtime will execute, but
the content of `shell` tool calls is determined by the LLM. Treat an agent you did
not write with the same caution you would treat untrusted code.

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
