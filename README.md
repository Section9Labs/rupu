# rupu

**Status:** CLI + control-plane web UI in active development, released on
`beta` and `stable` channels (`rupu update`). The native macOS app (`rupu.app`)
is deprecated — see [Install](#install).

---

## What is rupu?

`rupu` is a CLI for orchestrating coding and security agents across
repositories and hosts — driven by schedules and issue-tracker events, gated by
human approvals when you want them, with a JSONL transcript on every run. A
single Rust binary that:

- **Drives multiple LLM providers** — Anthropic, OpenAI, Gemini and GitHub
  Copilot via API key or SSO, plus any OpenAI-compatible `/v1/chat/completions`
  endpoint (vLLM, Oracle GenAI, Together, …). Named accounts let you hold
  several independently-credentialed identities per vendor (work / personal);
  credentials live in a chmod-600 `~/.rupu/auth.json` and SSO tokens refresh
  themselves.
- **Knows each model's limits** — every run resolves its model's context window
  and output cap (agent frontmatter → config → the provider's live model list),
  and `rupu models list|refresh` browses the discovered catalog.
- **Recovers from bad replies** — every provider reply and error is classified
  into a typed outcome (refusal, truncation, context overflow, rate limit, …)
  and walked up a recovery ladder: retry, then the agent's `fallbacks:` chain
  of other models/providers, then fail with a concrete hint. Anthropic
  server-side fallback is used where supported
  ([docs/response-outcomes.md](docs/response-outcomes.md)).
- **Resumes interrupted work** — `rupu run --continue <id>` rebuilds a killed or
  failed run's conversation from its transcript and carries on (optionally on
  another model); `rupu workflow resume` continues the steps and `for_each`
  units a dead runner left mid-flight instead of restarting them.
- **Runs agents and workflows** — agents are `.md` files with YAML frontmatter;
  workflows are YAML DAGs with linear steps, `for_each:` fan-out, `parallel:`
  and `panel:` review steps, `branch:` / `split:` / `join:` / `loops:`,
  deterministic `run:` command steps, `action:` connector steps, standalone
  approval gate nodes (with `notify:` hooks and unattended timeout routing),
  JSON-Schema `contracts:` between steps, and `rupu workflow run --file` for a
  workflow outside the catalog. `rupu agent create --describe` /
  `rupu workflow create --describe` have a model draft the definition for you.
- **Runs autonomously** — **autoflows** own issues end-to-end against
  persistent claim state (`rupu autoflow serve|tick|monitor`), and
  **agentiflows** run goal-directed, lead-coordinated agent fleets with a shared
  board, mailboxes, budgets and coverage-based stop conditions, steerable while
  they run (`rupu agentiflow run|attach|send|stop`).
- **Holds persistent sessions** — multi-turn agent conversations that outlive a
  single run, with compaction, archive/restore and attach
  (`rupu session`).
- **Fires on triggers** — cron schedules (`rupu cron tick` from system cron, no
  daemon), polled SCM events, or inbound webhooks from GitHub, GitLab, Linear
  and Jira (`rupu webhook serve`) ([docs/triggers.md](docs/triggers.md)).
- **Spans many hosts** — register remote hosts over SSH, HTTP, a dial-home
  WebSocket tunnel (`rupu node`) or a bucket dead-drop (`rupu node pull`), place
  a step with `host:` or spread a `for_each:` fan-out with `distribute:`, and
  get workspace changes synced back. SSH-hosted transcripts stream back through
  a lazy mirror, and remote units' coverage, findings and evidence artifacts
  reach the coordinator on every transport.
- **Serves a control-plane web UI** — `rupu cp serve` hosts a local dashboard:
  live runs and run graphs, transcripts, approvals, sessions, a workflow
  editor, findings, coverage, usage, network flows, hosts, agentiflows and
  customers, loading each host progressively.
- **Records coverage and structured findings** — an auditable coverage ledger
  of what the agent examined, for which concerns (OWASP / CWE / STRIDE / …),
  diff-able and replayable across runs; findings follow a schema-validated
  report contract (`full` / `summary` profiles) with evidence blocks (images,
  hexdumps, pcap refs) and a content-addressed artifact store; reports export
  to Markdown, HTML or PDF (`rupu findings export`) and older ones can be
  backfilled (`rupu findings import`) ([docs/coverage.md](docs/coverage.md)).
- **Lets you query and tag findings** — one query language
  (`severity>=high -tag:noise`) shared by the CLI, the web UI, agent tools and
  MCP, with a workspace-wide tag log (`rupu findings list|tag|tags`).
- **Speaks more than code** — data-driven engagement profiles (`code`, `web`,
  `api`, `network`, `binary`, `firmware`, `mobile`, `cloud`, `container`,
  `iac`, `sca`, `secrets`, `threat-model`, `redteam`, the `pentest` composite,
  or your own TOML) define the asset model findings and coverage are validated
  against (`--engagement-profile`;
  [docs/engagement-profiles.md](docs/engagement-profiles.md)).
- **Ships a stock security fleet** — `rupu fleet install` writes the generic,
  scope-driven agents and workflows the built-in profiles name in their
  `[bundle]` (recon, service analysis, exploit verification, web and API
  testing, code/SCA/secrets/IaC/cloud/container/binary/firmware/mobile review,
  threat modelling, red-team operations, and an `assessment-lead` for
  agentiflows) into `~/.rupu`, so an engagement has a fleet to draw on without
  hand-writing one.
- **Bills by customer** — group projects under customers with their own config
  layer (global → customer → project), attribute every run to one, and filter
  and price runs, usage and findings per customer in the control plane
  (`rupu customer`; [docs/cp-customers-api.md](docs/cp-customers-api.md)).
- **Counts every token** — a live per-run usage ledger with built-in pricing,
  Anthropic prompt caching, and reports grouped by provider, model, agent,
  workflow, repo or day (`rupu usage`).
- **Watches the network** — per-run netflow ledgers record rupu's own outbound
  HTTP and the sockets opened by an agent's `bash` subprocesses (passive,
  unprivileged OS socket observation on macOS and Linux), browsable in the web
  UI (`rupu netflow show`; [docs/netflow.md](docs/netflow.md)).
- **Names things for humans** — agent codenames (`cobalt-harbor/heron#412`)
  label runs, sessions, units and sub-agents across the CLI and web UI.
- **Keeps faithful transcripts** — schema v2 JSONL records thinking blocks,
  seeds, compaction and notices, and replays back to the exact conversation
  ([docs/transcript-schema.md](docs/transcript-schema.md)); runs render live in
  the terminal (a three-pane workflow dashboard) and `rupu watch <run_id>`
  re-attaches or replays.
- **Embeds an MCP server** — one typed GitHub + GitLab tool catalog for agents,
  also served to Claude Desktop / Cursor / any MCP host via `rupu mcp serve`
  ([docs/mcp.md](docs/mcp.md)).
- **Updates itself** — `rupu update` follows the `stable` or `beta` channel
  with checksum-verified, atomic in-place swaps.

What's NOT in this binary yet: the hosted multi-tenant `rupu.cloud` relay and
the remote sandbox runtime (Slice E). See [TODO.md](TODO.md) for deferred
items.

---

## Install

**From source (requires Rust 1.95+):**

```sh
cargo install --git https://github.com/Section9Labs/rupu
```

**Prebuilt binary:**

Download the binary for your platform from the
[Releases](https://github.com/Section9Labs/rupu/releases) page and place `rupu`
somewhere on your `$PATH`.

> **Not live yet.** The three community install paths below (AUR, Homebrew,
> Nix) become available with the first stable release after this lands — that
> release is what publishes `rupu-bin` to the AUR, pushes the formula to the
> tap, and fills in the flake's version and hashes. Until then, use the
> prebuilt binary or the `.deb`/`.rpm` below.

**Arch Linux (AUR):**

```sh
yay -S rupu-bin
# or: paru -S rupu-bin
```

`rupu-bin` installs the published release binary (not a from-source build)
and declares `ripgrep` as a dependency, matching the `.deb`/`.rpm` packages.

**Homebrew (macOS or Linux):**

```sh
brew install section9labs/tap/rupu
```

**Nix:**

```sh
nix run github:Section9Labs/rupu
# or, to install into a profile:
nix profile install github:Section9Labs/rupu
```

**Linux packages (.deb / .rpm):**

Debian/Ubuntu and Fedora/RHEL users can install a `.deb` or `.rpm` from the
same [Releases](https://github.com/Section9Labs/rupu/releases) page instead
of the bare binary. The package declares its own dependencies (ripgrep) and
installs shell completions (bash/zsh/fish) and a man page (`man rupu`)
alongside the binary — that's the reason to prefer it over the bare binary:

```sh
sudo apt install ./rupu_<version>_amd64.deb
# or — note the `-1` package release field in the .rpm name
sudo dnf install ./rupu-<version>-1.x86_64.rpm
```

A package-installed `rupu` defers upgrades to the package manager (`rupu
update` will tell you as much) rather than self-updating. To get upgrades via
`apt upgrade` / `dnf upgrade` instead of downloading a new `.deb`/`.rpm` by
hand every release, add the hosted repository once — see below.

**Hosted APT / YUM repositories:**

rupu publishes signed APT and YUM repositories with every release, so you
can `apt install`/`dnf install` and then upgrade in place with your normal
package manager. Both the `stable` and `beta` channels are hosted, but **do
not add both without pinning**: a beta version string like `0.70.4.beta`
sorts *higher* than `0.70.4` under both dpkg's and rpm's version comparison,
so with both channels enabled and unpinned, `apt`/`dnf` will pick beta on the
first upgrade and every upgrade after that keeps you on beta — silently, with
no warning. If you only ever want one channel, only add that one channel. If
you want both available but to stay on stable by default, pin as shown
below.

Everything is signed with key fingerprint
`6A2918F205000696D657AC61D672DF3BE13ADFDD`, whose public half is served from
a stable URL: `https://rupu.sh/rupu-archive-keyring.asc`.

Each channel's repository index carries **only the current release** —
it does not accumulate old versions. Pinning an older version means
downloading that release's `.deb`/`.rpm` directly from its
[GitHub release page](https://github.com/Section9Labs/rupu/releases) rather
than `apt install rupu=<old-version>`, which will not find it in the index.

*Debian/Ubuntu — stable, deb822 format (modern `apt`, `.sources` file):*

```sh
sudo install -m 0755 -d /etc/apt/keyrings
sudo curl -fsSL -o /etc/apt/keyrings/rupu.asc \
  https://rupu.sh/rupu-archive-keyring.asc
sudo chmod 644 /etc/apt/keyrings/rupu.asc
sudo tee /etc/apt/sources.list.d/rupu.sources > /dev/null <<'EOF'
Types: deb
URIs: https://rupu.sh/apt
Suites: stable
Components: main
Architectures: amd64 arm64
Signed-By: /etc/apt/keyrings/rupu.asc
EOF
sudo apt update && sudo apt install rupu
```

*Debian/Ubuntu — stable, legacy one-line format (older Ubuntu LTS releases
whose `apt` doesn't understand deb822 `.sources` files):*

```sh
sudo install -m 0755 -d /etc/apt/keyrings
sudo curl -fsSL -o /etc/apt/keyrings/rupu.asc \
  https://rupu.sh/rupu-archive-keyring.asc
sudo chmod 644 /etc/apt/keyrings/rupu.asc
echo "deb [signed-by=/etc/apt/keyrings/rupu.asc] https://rupu.sh/apt stable main" \
  | sudo tee /etc/apt/sources.list.d/rupu.list
sudo apt update && sudo apt install rupu
```

*Fedora/RHEL — stable:*

```sh
sudo rpm --import https://rupu.sh/rupu-archive-keyring.asc
sudo tee /etc/yum.repos.d/rupu.repo > /dev/null <<'EOF'
[rupu]
name=rupu
baseurl=https://rupu.sh/yum/stable
enabled=1
gpgcheck=1
repo_gpgcheck=1
gpgkey=https://rupu.sh/rupu-archive-keyring.asc
EOF
sudo dnf install rupu
```

*Beta channel:* identical snippets with `stable` → `beta` — in the `Suites:`
line and `deb` line for APT, and in the `baseurl` for YUM — and nothing else.
The key, import commands, and package name are unchanged.

**Do not add both channels without pinning.** Version strings compare as
`0.70.4.beta` > `0.70.4` under both dpkg's and rpm's version comparison, so
with both sources enabled and unpinned, `apt upgrade`/`dnf upgrade` will pick
beta the first time it's available and every upgrade after that keeps you on
beta — silently, with no warning.

```sh
# APT (deb822), beta — self-contained
sudo install -m 0755 -d /etc/apt/keyrings
sudo curl -fsSL -o /etc/apt/keyrings/rupu.asc \
  https://rupu.sh/rupu-archive-keyring.asc
sudo chmod 644 /etc/apt/keyrings/rupu.asc
sudo tee /etc/apt/sources.list.d/rupu-beta.sources > /dev/null <<'EOF'
Types: deb
URIs: https://rupu.sh/apt
Suites: beta
Components: main
Architectures: amd64 arm64
Signed-By: /etc/apt/keyrings/rupu.asc
EOF
```

If you want beta available alongside stable but to stay on stable by
default, pin stable above beta. The repo metadata carries the fields needed
to pin precisely — `Origin: Section9Labs`, `Label: rupu`, and
`Suite`/`Codename` equal to the channel name:

```sh
sudo tee /etc/apt/preferences.d/rupu > /dev/null <<'EOF'
Package: *
Pin: release o=Section9Labs, l=rupu, a=stable
Pin-Priority: 900

Package: *
Pin: release o=Section9Labs, l=rupu, a=beta
Pin-Priority: 100
EOF
```

With that pin in place, `apt install rupu` / `apt upgrade` always resolves to
stable even with both sources enabled. Pull a beta build anyway, on demand,
with `apt install -t beta rupu`.

For YUM/DNF there is no equivalent pinning mechanism, so ship the beta repo
**disabled** and opt in per command instead:

```sh
# YUM/DNF, beta — disabled by default
sudo tee /etc/yum.repos.d/rupu-beta.repo > /dev/null <<'EOF'
[rupu-beta]
name=rupu (beta)
baseurl=https://rupu.sh/yum/beta
enabled=0
gpgcheck=1
repo_gpgcheck=1
gpgkey=https://rupu.sh/rupu-archive-keyring.asc
EOF
```

`dnf install rupu` / `dnf upgrade` then always stay on stable. Pull a beta
build on demand with:

```sh
sudo dnf install --enablerepo=rupu-beta rupu
sudo dnf upgrade --enablerepo=rupu-beta rupu
```

**macOS app (rupu.app) — deprecated:**

The native SwiftUI client, `rupu.app`, is deprecated and no longer developed.
Its last builds remain on the
[Releases](https://github.com/Section9Labs/rupu/releases) page, but new
features land only in the CLI and the control-plane web UI — run
`rupu cp serve` and use the browser instead.

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

rupu ships five LLM providers. Four support both API-key and SSO auth:

| Provider  | API key                              | SSO                                |
| --------- | ------------------------------------ | ---------------------------------- |
| anthropic | `console.anthropic.com` → API Keys   | Claude.ai login (browser callback) |
| openai    | `platform.openai.com` → API Keys     | ChatGPT login (browser callback)   |
| gemini    | `aistudio.google.com` → Get API Key  | Google account (browser callback)  |
| copilot   | (PAT via `gh` token)                 | GitHub login (device code)         |

A fifth, `openai-compatible`, is a user-declared generic adapter for any
`/v1/chat/completions` endpoint (vLLM, Oracle GenAI, Together, …) — API-key
auth only, no SSO. See `docs/providers.md` and
`docs/providers/openai-compatible.md`.

```sh
# API key
rupu auth login --provider anthropic --mode api-key --key sk-ant-XXX

# SSO (opens a browser; Copilot prints a device code instead)
rupu auth login --provider anthropic --mode sso

# Verify
rupu auth status
```

Credentials are stored at `~/.rupu/auth.json` (a chmod-600 file, the only
store — matches `gh`, `aws`, `gcloud`; `rupu auth backend` reports where it is).
SSO entries auto-refresh near expiry;
failure surfaces an actionable error pointing at `rupu auth login --mode sso`.

`--provider` above is an alias for `--account` — one credential per
vendor is the default, but `--account <name> --kind <vendor>` (e.g.
`--account anthropic-work --kind anthropic`) declares a second,
independently-credentialed account of the same vendor. See
`docs/providers.md` for the full reference and `docs/providers/<name>.md`
for per-provider walkthroughs.

## SCM & issue trackers

rupu integrates with GitHub and GitLab through a single embedded MCP
server. Agents call typed tools (`scm.prs.diff`, `issues.get`, ...) and
the right per-platform connector dispatches the call. See `docs/scm.md`
for the full reference — including `--account`/`--kind` for a second
account of the same platform (e.g. work vs. personal GitHub, or
github.com alongside a GitHub Enterprise host) and `rupu scm bind` /
`rupu scm accounts` for routing repos to the right one via
`[[scm.rules]]`.

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
| `~/.rupu/workspaces/` | Workspace (project) records + customer assignments |
| `~/.rupu/runs/` | Persistent workflow run store (status, step results, events) |
| `~/.rupu/sessions/` | Persistent agent sessions |
| `~/.rupu/agentiflows/` | Agentiflow definitions and run directories |
| `~/.rupu/customers/` | Customer records and per-customer config layers |
| `~/.rupu/findings/artifacts/` | Content-addressed finding artifact store |
| `~/.rupu/hosts/` | Registered remote hosts |

### Per-project (`<project>/.rupu/`)

| Path | Purpose |
|------|---------|
| `<project>/.rupu/agents/` | Agent `.md` files for this repo |
| `<project>/.rupu/contracts/` | Repo-local JSON Schemas for workflow handoffs |
| `<project>/.rupu/workflows/` | Workflow YAML files for this repo |
| `<project>/.rupu/agentiflows/` | Agentiflow definitions for this repo |
| `<project>/.rupu/coverage/` | Coverage ledgers, findings and the finding tag log |
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

Full documentation lives at **<https://rupu.sh/docs/>**. The same material, in
this repo:

- [`docs/using-rupu.md`](docs/using-rupu.md) — practical day-to-day usage
- [`docs/cli-reference.md`](docs/cli-reference.md) — every command, argument and flag (generated from `rupu --help` by `scripts/gen-cli-reference.py`)
- [`docs/agent-format.md`](docs/agent-format.md) — complete agent schema reference
- [`docs/agent-authoring.md`](docs/agent-authoring.md) — how to write good agents
- [`docs/workflow-format.md`](docs/workflow-format.md) — complete workflow schema reference
- [`docs/workflow-authoring.md`](docs/workflow-authoring.md) — how to design good workflows
- [`docs/agentiflows.md`](docs/agentiflows.md) — goal-directed agent fleets (`rupu agentiflow`)
- [`docs/triggers.md`](docs/triggers.md) — cron, polled-event and webhook triggers
- [`docs/configuration.md`](docs/configuration.md) — complete `config.toml` reference, including the customer layer
- [`docs/providers.md`](docs/providers.md) — LLM providers, accounts and auth modes (per-provider pages in [`docs/providers/`](docs/providers/))
- [`docs/response-outcomes.md`](docs/response-outcomes.md) — reply outcomes, the recovery ladder and `fallbacks:`
- [`docs/scm.md`](docs/scm.md) — GitHub / GitLab / issue-tracker integration (per-platform pages in [`docs/scm/`](docs/scm/))
- [`docs/mcp.md`](docs/mcp.md) — the embedded MCP server and its tool catalog
- [`docs/coverage.md`](docs/coverage.md) — coverage harness, finding reports, queries, tags and exports
- [`docs/engagement-profiles.md`](docs/engagement-profiles.md) — engagement profiles: asset kinds, the built-in catalog, writing your own, the asset ledger, the stock fleet
- [`docs/netflow.md`](docs/netflow.md) — per-run network flow capture: what is recorded, ledgers, ASN enrichment, `rupu netflow`, `[netflow]` config
- [`docs/cp-customers-api.md`](docs/cp-customers-api.md) — the control plane's customers API and web scope
- [`docs/transcript-schema.md`](docs/transcript-schema.md) — the JSONL transcript event schema
- [`docs/development-flows.md`](docs/development-flows.md) — recommended engineering flows
- [`docs/spec.md`](docs/spec.md) — architecture reference
- [`docs/RELEASING.md`](docs/RELEASING.md) — release channels and the release-gated CI
- [`examples/README.md`](examples/README.md) — copyable agents and workflows

## Subcommands

```
rupu init [--with-samples] [--git]    Bootstrap .rupu/ in the current dir
rupu run <agent> [prompt|target]      Run an agent (--continue <id>, --model, --provider,
                                       --engagement-profile, --findings-profile, --tmp/--into)
rupu run {list, show, pause, resume}  List / inspect runs; pause or resume a run by id
rupu agent {list, show, edit, create} Manage agents (create --describe drafts one with a model)
rupu workflow {list, show, edit, create}
                                       Manage workflows (create --describe drafts one with a model)
rupu workflow run <name> [target]     Run a workflow (target: repo, PR, or issue ref; --file <path>)
rupu workflow {runs, show-run}        List / inspect persisted runs
rupu workflow {approve, reject} <id>  Release / reject a parked approval gate (--gate <step-id>)
rupu workflow {cancel, pause, resume} Control a run; resume continues interrupted work
rupu workflow {archive-run, restore-run, delete-run}
                                       Manage run history
rupu agentiflow {run, list, status, attach, send, stop, serve}
                                       Goal-directed, lead-coordinated agent fleets
rupu autoflow {list, show, run, tick, serve, stop, monitor, history, ...}
                                       Autonomous workflows against persistent issue state
                                       (+ wakes, explain, doctor, repair, requeue, status,
                                        claims, release, create)
rupu session {start, list, show, send, attach, stop, compact, usage-timeline,
              archive, restore, delete, prune}
                                       Persistent agent sessions (multi-turn conversations)
rupu watch <run_id> [--follow|--replay]
                                       Re-attach to any past or in-flight run
rupu transcript {list, show, archive, delete, prune}
                                       Browse and manage JSONL transcripts
rupu coverage {list, show, audit, gap, catalog, templates}
                                       Inspect agentic coverage ledgers and concern catalogs
rupu coverage {runs, diff, rerun}     Compare and replay coverage runs
rupu findings {list, tag, tags}       Query findings (`severity>=high -tag:noise`) and tag them
rupu findings {export, import, schema}
                                       Export reports (--to md|html|pdf), backfill old ones,
                                       print the report JSON Schema
rupu netflow {show, prune}            Per-run network flow ledgers
rupu fleet {install, list}            Install the stock security-assessment fleet (--force, --project)
rupu usage [runs, backfill]           Token spend + cost reports (--group-by, --since, filters)
rupu cleanup [--sessions|--transcripts] [--stats] [--dry-run]
                                       Prune archived local sessions and transcripts
rupu issues {list, show, run}         Issue-tracker surface (auto-detects from cwd)
rupu repos {list, attach, prefer, tracked, forget}
                                       SCM repositories and tracked local checkouts
rupu scm {bind, accounts}             Multi-account SCM routing rules and account roster
rupu cron {list, tick, events}        Cron + polled-event trigger runtime
rupu webhook serve [--addr]           Long-lived webhook receiver for GitHub / GitLab / Linear / Jira
rupu mcp serve [--transport]          Expose rupu's tools to MCP clients
rupu cp serve [--bind] [--token]      Local control-plane HTTP server for the rupu web UI
rupu host {add, list, remove}         Manage named remote hosts (SSH / HTTP / tunnel / bucket)
rupu node [--cp-url] | {enroll, pull} Dial-home tunnel agent, node enrollment, bucket worker
rupu auth {login, logout, status, backend}
                                       Provider + SCM credential management
rupu models {list, refresh}           Browse / refresh discovered model lists and limits
rupu customer {list, show, create, set, edit, archive, unarchive, delete, assign, unassign}
                                       Customers: project grouping + a per-customer config layer
rupu config {get, set}                Read / write rupu configuration
rupu ui {themes, theme {show, validate, import}}
                                       List, inspect, validate, and import UI themes
rupu completions {print, install}     Shell-completion scripts (with dynamic agent names)
rupu man                              Print the man page (roff) to stdout
rupu update [--check] [--channel]     Download and install the latest release for the configured channel
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

- `rupu auth backend` (`table` and `json`)
- `rupu workflow show` (`table` and `json`)
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

`rupu workflow show` now uses a retained static definition snapshot for human terminal output.
The default table/human surface keeps workflow summary metadata together with a graph-style step
preview. When `--view` is omitted, `workflow show` defaults to `full`. Use
`--view focused|compact|full` to control how much definition detail is shown:
`focused` keeps the summary plus graph, `compact` adds declared inputs/outputs and step details in
table form, and `full` adds the raw highlighted YAML below the definition snapshot.

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
- **Workflows** are YAML orchestration DAGs: linear steps plus `for_each`,
  `parallel`, `panel`, `branch`, `split`/`join`, `loops`, `run:` command steps,
  `action:` connector steps, approval gate nodes, remote placement
  (`host:` / `distribute:`), and trigger support.
- **Autoflows** are workflows with an `autoflow:` block that own issues
  end-to-end against persistent claim state; **agentiflows** are YAML goal
  definitions run by a lead agent that dispatches agent and workflow units
  over a shared fleet board until goals, coverage or a budget stop it.
- **Transcripts** are append-only JSONL files, and workflow runs are also tracked in
  the persistent run store for re-attach, approval, and history.
- **Sessions** are persistent agent containers that own multiple standalone runs over
  time; use `rupu session start`, `rupu session send`, and `rupu session attach` for
  long-lived agent conversations. Sessions can also be archived, restored, listed with
  `--all|--archived`, pruned by age, permanently deleted with `--force`, or cleaned in
  bulk with `rupu cleanup`.
- **Tool policy** lives in each agent's `tools:` and `permissionMode`. A workflow
  step's `actions:` further narrows the connector/MCP subset of that grant for
  the step — it never touches builtin tools (`bash`, `read_file`, `write_file`,
  `edit_file`, `grep`, `glob`, `ast_grep`, `dispatch_agent`,
  `dispatch_agents_parallel`) and can only narrow, never grant beyond what the
  agent's `tools:` already allows. Every catalog call is recorded in the
  transcript's `tool_audit` trail.

### Crates

The workspace is hexagonal: `rupu-providers`, `rupu-tools` and `rupu-auth`
define ports, the agent runtime only knows traits, and `rupu-cli` is a thin
clap dispatcher.

| Crate | Role |
|-------|------|
| `rupu-cli` | The `rupu` binary — argument parsing and delegation to the libraries |
| `rupu-agent` | Agent file format, agent loop, permission resolver, continuation/replay, outcome classification and the recovery ladder |
| `rupu-providers` | LLM provider clients behind the `LlmProvider` port, typed reply outcomes, model limits |
| `rupu-runtime` | Run assembly shared by CLI, orchestrator and CP: provider factory, model-limit resolution, fallback hops, credential manifest |
| `rupu-auth` | Credential storage (`~/.rupu/auth.json`, mode 0600) and SSO flows |
| `rupu-tools` | Builtin agent tools: `bash`, file read/write/edit, `grep`, `glob`, `ast_grep`, sub-agent dispatch |
| `rupu-ast` | Tree-sitter CST wrapper behind `ast_grep` source and CST previews |
| `rupu-orchestrator` | Workflow YAML parser, minijinja rendering, DAG executor, gates, actions, run store and interrupt recovery |
| `rupu-agentiflow` | The agentiflow envelope: definition loader, round loop, lead driver, budgets and stop conditions |
| `rupu-fleet` | File-backed fleet comms for agentiflows: shared board (claims / posts / directives) and mailboxes |
| `rupu-transcript` | JSONL transcript event schema (v2), writer and reader |
| `rupu-codename` | Human codenames for runs, sessions, units and sub-agents |
| `rupu-config` | Layered TOML configuration (global → customer → project) |
| `rupu-workspace` | Workspace records, customer store, config-layer resolution, workspace sync deltas |
| `rupu-coverage` | Coverage ledgers, concern catalogs, engagement profiles, finding reports, query language and tags |
| `rupu-findings-report` | Pure finding-report renderers (Markdown / HTML / PDF via Typst) and the Markdown importer |
| `rupu-scm` | GitHub / GitLab repo + issue connectors and event pollers |
| `rupu-mcp` | Embedded MCP server (in-process and stdio) over the SCM tool catalog |
| `rupu-webhook` | Webhook receiver for event-triggered workflows (GitHub, GitLab, Linear, Jira) |
| `rupu-cp` | Control-plane HTTP server, host connectors (local / SSH / HTTP / tunnel / bucket) and the embedded web UI |
| `rupu-netflow` | Network egress observability: per-run flow ledgers for rupu's own HTTP |
| `rupu-netwatch` | Passive subprocess socket capture (macOS / Linux) for agent `bash` calls |
| `rupu-app-canvas` | Pure view layer that turns a workflow into graph rows for the CLI's workflow views |
| `rupu-update` | Channel-aware release selection, checksum verification and atomic binary swap behind `rupu update` |

---

## Agents are code

Bypass mode runs arbitrary shell commands on your machine. Review every agent file
before you run it, just as you would review a shell script. An agent's `tools:` list
and `permissionMode` define its tool surface; a workflow step's `actions:` can only
narrow that surface's connector/MCP tools further, never widen it or restrict
builtins — it is not a substitute for reviewing the agent's own `tools:` and
`permissionMode`. Treat an agent you did not write with the same caution you would
treat untrusted code.

---

## Hacking / development

```sh
git clone https://github.com/Section9Labs/rupu
cd rupu
cargo build --workspace
cargo test --workspace
```

MSRV: **1.95**. Set `RUPU_LOG=debug` for verbose tracing output.

### Tests

Each crate's integration tests are modules of **one** test binary,
`crates/<crate>/tests/it/main.rs`. Every top-level `tests/*.rs` file would be a
separate binary that links the whole dependency graph; building, linking and
(on macOS) first-launching ~200 of them was most of what `cargo test
--workspace` spent its time on. Add a test file as
`tests/it/<name>.rs` plus a `mod <name>;` line in `main.rs`;
`crates/rupu-cli/tests/it/test_layout.rs` fails on a new top-level file. Run one
file's tests with `cargo test -p rupu-cp --test it host_reads::`.

Those tests share a process, so a test that changes env vars or the working
directory must not overlap the others. Mark it `#[serial]` (`serial_test`); in
`rupu-cli`, put it in `tests/serial/` instead, holding `ENV_LOCK` for the whole
test. That binary runs one test at a time and restores env and cwd after each.

Pass `</dev/null` when running the suite from an interactive shell: two CLI
approval-prompt tests block on an open stdin.

### Faster local builds

- **Linux:** `.cargo/config.toml` links through `scripts/fast-link.sh`, which
  uses [mold](https://github.com/rui314/mold) when both `mold` and `clang` are
  installed (CI's test job links with mold too) and the toolchain default
  otherwise. Set `RUPU_NO_FAST_LINKER=1` to turn it off.
- **macOS:** no override; Apple's default linker (ld-prime) stays. rust-lld
  (LLD 22) can't link against the macOS 27 SDK at all, because it rejects the
  SDK's `arm64e.x1` TBD entries. Pointed at the 26.5 SDK it rebuilt only ~2 s
  faster (median 18 s vs 20.5 s after touching `rupu-providers`), with no gain
  on a full `cargo test` run.
- **macOS first-run scan:** macOS security-scans every newly linked executable
  the first time it runs, about 3–4 s for each of the large test binaries here,
  after every rebuild. Adding your terminal app under System Settings → Privacy
  & Security → Developer Tools exempts programs it launches from that scan. That
  is a security trade-off; weigh it before you make it.

---

## License

[Apache-2.0](LICENSE)
