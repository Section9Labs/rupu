# Configuration Reference

> See also: [providers.md](providers.md) · [scm.md](scm.md) · [using-rupu.md](using-rupu.md)

Complete reference for `~/.rupu/config.toml` (global), `~/.rupu/customers/<slug>/config.toml`
(customer-specific), and `<repo>/.rupu/config.toml` (project-local override). This page enumerates every key `rupu-config` accepts; for
narrative walkthroughs of the provider and SCM sections, see the linked docs above.

---

## Locations and layering

- `~/.rupu/config.toml` — global config.
- `~/.rupu/customers/<slug>/config.toml` — customer-specific config (see *Customer layer* below).
- `<project>/.rupu/config.toml` — project-local overrides. Scalars and tables override the
  global value; arrays replace rather than merge.
- Every section is `deny_unknown_fields`: an unrecognized key fails to parse. Three
  keys are accepted-but-inert exceptions to this — see **Deprecated keys** below — kept
  as opaque no-op shims so an old config doesn't lose every other setting it carries.
- All fields are optional. A missing value at one layer can be supplied by another;
  the actual defaults applied are documented per key below.

### Customer layer

A project can be assigned to a **customer** (`rupu customer assign <slug>`; a slug
is 1–63 of `a-z`, `0-9`, `-`, and `none` is reserved — it is the "no customer"
filter). The customer's layer, `~/.rupu/customers/<slug>/config.toml`, sits between the global
file and the project's `.rupu/config.toml`:

global lock › customer lock › project › customer › global › default

- The project still wins on any key the customer has not locked.
- A customer locks keys with its own `[policy].lock` — e.g. `["default_provider"]`
  so a repo's `.rupu/config.toml` cannot move the customer's runs onto another account.
  A lock covers config keys only: an agent whose own frontmatter names `provider:` /
  `auth:` still runs on that provider. It cannot unlock a key the global
  `[policy].lock` names.
- Arrays replace, as between global and project: a customer that declares
  `[[scm.rules]]` replaces the global rules for its projects.
- A run uses the customer of the nearest assigned ancestor of its working directory —
  so subdirectories inherit their project's customer, and a repo needs no `.rupu/` of its own.
  An autoflow run uses its repo's checkout (not its issue worktree).
- A run **records** the customer it ran under (`run.json`, every transcript's `RunStart`,
  and a session's `session.json`) — `"customer": null` when it ran with none.
  `rupu workflow resume` / `approve` run on what was recorded even if the project has
  since been reassigned: the recorded customer, or no customer for a recorded `null`
  (even if the project has been assigned since). A recorded customer that no longer
  exists fails the resume. Only a run from before customers existed (no `customer` key
  at all) falls back to the directory it was launched from, and shows the project's
  *current* customer, marked derived in the CP API
  ([`cp-customers-api.md`](cp-customers-api.md)).
- A project assigned to a customer that no longer exists, or a customer layer that
  does not parse, fails the run — it never falls back to the global config.

Typical customer layer:

    default_provider = "anthropic-acme"

    # optional — already declared globally by `auth login --account anthropic-acme --kind anthropic`
    [providers.anthropic-acme]
    kind = "anthropic"

    [[scm.rules]]
    owner = "acme-corp"
    account = "github-acme"

    [policy]
    lock = ["default_provider"]

Assign from the repo root: `rupu customer assign acme` (assigns the current directory).
Each account must be declared once in the global config: `rupu auth login --account anthropic-acme --kind anthropic` for a provider account,
or `rupu auth login --account github-acme --kind github` for an SCM account. The customer layer then references them via `default_provider` or `[[scm.rules]]`.

To view the effective config with sources and locks: `rupu customer show <slug>`.
To edit and validate a customer layer: `rupu customer edit <slug>`.

The control plane manages customers, filters its lists by `?customer=` and previews a launch's accounts — see [`cp-customers-api.md`](cp-customers-api.md).

---

### Environment variables

There is no environment *layer*: environment variables are not merged into the
config. A few are read directly by the code they affect and override the matching
key there:

| Variable | Overrides / does |
|----------|------------------|
| `RUPU_HOME` | Moves the global directory (default `~/.rupu`) — config, credentials, runs, caches |
| `RUPU_LOG` | Wins over `log_level` |
| `RUPU_MAX_OPEN_FILES`, `RUPU_MAX_CONCURRENT_JOBS`, `RUPU_MIN_FREE_MEMORY_MB` | Win over the `[runtime]` keys of the same name (`--max-open-files` also wins over `max_open_files`) |
| `RUPU_NETFLOW_SUBPROCESS=0` | Forces `[netflow].subprocess_capture` off |
| `RUPU_NO_UPDATE_CHECK` | When set to any value (even `0`), suppresses the passive update notice, like `[update].check = false` |
| `RUPU_LIVE_VIEW=0` | Turns the workflow live view off |
| `NO_COLOR` | Forces `[ui].color = "never"` |
| `VISUAL` / `EDITOR` | Used when `[ui].editor` is unset |
| `RUPU_<ACCOUNT>_API_KEY` | API-key fallback for a provider account with no stored credential (see [providers.md](providers.md)) |

### Editing

`rupu config get <key>` / `rupu config set <key> <value>` read and write the
**global** `config.toml` only. Dotted keys descend into tables (`ui.theme`,
`cp.gate_sweep_interval_secs`); the value is parsed as a TOML scalar (string,
integer or bool), and `set` refuses to replace a table with a scalar or the
reverse. Arrays and tables (`[[scm.rules]]`, `[recovery].fallbacks`) are edited
by hand, with `rupu customer edit <slug>` for a customer layer, or from the
control plane's Settings page (effective values with the layer each came from,
lock toggles, and a raw TOML editor).

---

## Top-level keys

| Key               | Type   | Default             | Notes                                                        |
|-------------------|--------|----------------------|---------------------------------------------------------------|
| `default_provider` | string | none                 | Provider used when an agent file omits `provider:`            |
| `default_model`     | string | `claude-sonnet-4-6`  | Model used when an agent file omits `model:` and `[providers.<name>].default_model` is also unset |
| `permission_mode`   | string | `ask`                | Fallback permission mode when neither the agent nor `--mode` sets one |
| `log_level`         | string | `warn` | Logging verbosity — any `tracing-subscriber` directive (`debug`, `rupu_scm=debug,info`). `RUPU_LOG` wins over it; a blank value counts as unset |

---

## `[bash]`

| Key             | Type            | Default                          | Notes |
|-----------------|-----------------|-----------------------------------|-------|
| `timeout_secs`  | integer         | `120`                             | Timeout for a single `bash` tool invocation |
| `env_allowlist` | array\<string\> | `[]`                              | Extra env vars forwarded into the bash subprocess, beyond the always-allowed `PATH`/`HOME`/`USER`/`TERM`/`LANG` |

---

## `[providers.<name>]`

One table per provider **account**: a bare vendor name (`anthropic`, `openai`,
`gemini`, `copilot`), a named account of a vendor (`anthropic-work` with
`kind = "anthropic"`), or a user-declared `openai-compatible` name such as `oracle`.
An agent's `provider:` names the account. Full narrative reference:
[providers.md](providers.md#field-reference).

| Key               | Type                     | Default                                             |
|-------------------|--------------------------|-------------------------------------------------------|
| `base_url`        | string                   | vendor's documented URL; today read only by `openai-compatible` accounts — anthropic-kind ignores it (I-92) |
| `kind`            | string                   | none — the table name is the vendor (`[providers.anthropic]`). Set it to name the vendor of a second, named account (`[providers.anthropic-work] kind = "anthropic"`; written for you by `rupu auth login --account <name> --kind <vendor>`), or `"openai-compatible"` to declare a generic adapter. See [providers.md](providers.md#accounts-vs-vendor-kind) |
| `stream`          | bool                     | `true`                                                 |
| `org_id`          | string                   | none (OpenAI-only; sent as `OpenAI-Organization`)      |
| `region`          | string                   | none (accepted but not currently used by any shipped client) |
| `timeout_ms`      | integer                  | `120000` (2 min); `0` treated as unset                 |
| `max_retries`     | integer                  | `1` (retries after the first attempt on a retryable error) |
| `max_concurrency` | integer                  | anthropic `4`, openai `8`, gemini `4`, copilot `4`; `0` treated as unset |
| `prompt_cache`    | bool                     | `true` (Anthropic only; `false` for gateways that reject `cache_control` — gateway routing is process-wide today, see [providers.md](providers.md#field-reference); agent `anthropicPromptCache` overrides) |
| `default_model`   | string                   | none — the agent must set `model:` or rely on this      |
| `models`          | array\<table\>           | `[]` — each entry: `id` (required), `context_window` (input-token limit) and `max_output` (both optional; when omitted, or `0`, the limit is discovered from the provider's model list, else unknown — see [providers.md](providers.md#model-limits)) |

An `openai-compatible` entry additionally requires `base_url` and `default_model`
(validated at load time), and may not reuse a reserved built-in provider name
(`anthropic`, `openai`, `gemini`, `copilot`, `local`, `github`, `gitlab`, `linear`,
`jira`).

---

## `[scm]` / `[issues]`

Full narrative reference: [scm.md](scm.md#configuration).

| Key                  | Type   | Default | Notes |
|----------------------|--------|---------|-------|
| `[scm.default]`      | table  | none    | `platform` — fallback platform when a tool call omits `platform?`. `owner` / `repo` are deprecated and never read (a warning at load says to delete them) |
| `[issues.default]`   | table  | none    | `tracker`, `project` — fallback tracker when a tool call omits `tracker?` |
| `[scm.<account>]`    | table  | none    | One table per SCM account. The table name is the account (`[scm.github]`, `[scm.gh-work]`). Keys: `kind` (`"github"` \| `"gitlab"`; omit when the name *is* the platform), `base_url`, `timeout_ms` default `30000`, `max_concurrency` default github `8` / gitlab `6`, `clone_protocol` default `https`; GitLab only: `oauth_client_id`, the OAuth application SSO logs in as — default glab's on gitlab.com, required for self-managed |
| `[[scm.rules]]`      | array  | `[]`    | Account selection when two accounts share a platform: each entry sets `account` plus exactly one of `owner` (repo-owner glob, `acme/*`) or `path` (cwd glob, `~/Code/work/*`). Precedence: `--account` → owner rule → path rule → the sole account of that platform → error. Append one with `rupu scm bind`; list accounts with `rupu scm accounts`. See [scm.md](scm.md#multi-account-routing) |

---

## `[ui]`

| Key                | Type   | Default              | Notes |
|--------------------|--------|----------------------|-------|
| `color`            | string | `auto`               | `auto` \| `always` \| `never` |
| `theme`            | string | none                 | Shared syntax+palette theme selector |
| `live_view`        | string | `focused`            | `focused` \| `compact` \| `full` |
| `pager`            | string | `auto`               | `auto` \| `always` \| `never` |
| `editor`           | string | `$VISUAL`/`$EDITOR`  | Command used by `agent edit`/`create`, `workflow edit`/`create` |
| `[ui.syntax].theme`  | string | `base16-ocean.dark`  | syntect theme name |
| `[ui.palette].theme` | string | `rupu-dark`          | Named rupu CLI palette |
| `[ui.cp].shell`      | string | `v1`                 | `v1` \| `v2` — CP web shell generation (Shell v2 redesign). Requires rupu ≥ the version this lands in: older binaries reject unknown `[ui]` keys and silently fall back to a default config. |

---

## `[triggers]`

| Key                   | Type            | Default | Notes |
|-----------------------|-----------------|---------|-------|
| `poll_sources`        | array\<string \| table\> | `[]` | Repo (`github:owner/repo`, `gitlab:group/project`) or tracker-native (`linear:<team-id>`, `jira:<site>/<project>`) sources. A table entry (`{ source = "…", poll_interval = "5m", account = "…" }`) adds a per-source `poll_interval` and an explicit `account` — the only way to pick between two tracker accounts of one kind; repo sources otherwise resolve their account through `[[scm.rules]]` |
| `max_events_per_tick` | integer         | `50`    | Cap on events processed per source per `rupu cron tick` pass |

---

## `[autoflow]`

Narrative reference: [using-rupu.md](using-rupu.md#autoflow-mode).

| Key                | Type    | Default     | Notes |
|--------------------|---------|-------------|-------|
| `enabled`          | bool    | `false`     | |
| `repo`             | string  | none        | e.g. `github:your-org/your-repo` |
| `checkout`         | string  | `worktree`  | `worktree` \| `in_place` |
| `worktree_root`    | string  | none        | Only meaningful when `checkout = "worktree"` |
| `permission_mode`  | string  | none        | Only `bypass` or `readonly` are accepted for autoflow execution — `ask` and any other value are rejected |
| `strict_templates` | bool    | `false`     | |
| `max_active`       | integer | none (unbounded) | Cap on concurrently active autoflow claims |
| `cleanup_after`    | string  | none (never pruned) | e.g. `7d` — completed/released claims and their worktrees are pruned by a later `rupu autoflow tick` once elapsed |

---

## `[pricing]`

Consumed by `rupu usage` and `rupu workflow runs` to convert token counts into a USD
figure. Three lookup tiers, in order: (1) `[pricing.<provider>."<model>"]` user
override, (2) a built-in defaults table for major models, (3) `[pricing.agents.<agent-name>]`
fallback when no model-level price is known — the hatch for a private/internal endpoint
with no public pricing (e.g. an `openai-compatible` provider, which otherwise reports
`$0.00`).

`<provider>` is whatever the run recorded: a vendor key (`anthropic`), a friendly alias
(`openai`, `gemini`, `copilot`), or a **named account** from `[providers.<name>]`
(`openai-oracle`). Named accounts resolve through their `kind` — a run on
`openai-oracle` (`kind = "openai"`) prices from the OpenAI table with no extra config.
Precedence among user overrides is account first, then vendor: `[pricing.openai-oracle."gpt-5.6-cyber"]`
beats `[pricing.openai."gpt-5.6-cyber"]`, which in turn applies to every account of that
kind. An account with no `kind` is the vendor itself. `google-antigravity` has no table of
its own and falls back to the Gemini rates.

Model ids are matched exact first, then with a trailing `[tag]` stripped
(`claude-sonnet-4-6[1m]` → `claude-sonnet-4-6`), then with a date snapshot stripped —
both OpenAI's `-YYYY-MM-DD` and Anthropic's compact `-YYYYMMDD`. The built-in table
carries each vendor's standard-tier, short-context (≤200k prompt) list rate; long-context
surcharges (OpenAI, Gemini Pro), batch/flex tiers, and regional uplifts are not modeled.
Anthropic prompt-cache writes are modeled: the built-in Anthropic entries bill them at
1.25x the input rate (the 5-minute TTL, rupu's only TTL). Override in config when accuracy matters.

```toml
[pricing.anthropic."claude-sonnet-4-6"]
input_per_mtok = 3.0
output_per_mtok = 15.0
cached_input_per_mtok = 0.30   # optional; omit to bill cached tokens at the full input rate
cache_write_per_mtok = 3.75    # optional; omit to bill cache writes at the full input rate

[pricing.openai."gpt-5"]
input_per_mtok = 1.25
output_per_mtok = 10.0

[pricing.agents.security-reviewer]
input_per_mtok = 3.0
output_per_mtok = 15.0
```

| Field                    | Type   | Required | Default | Notes |
|--------------------------|--------|:--------:|---------|-------|
| `input_per_mtok`         | float  | yes      | —       | USD per million input tokens |
| `output_per_mtok`        | float  | yes      | —       | USD per million output tokens |
| `cached_input_per_mtok`  | float  | no       | falls back to `input_per_mtok` | USD per million cached-input (cache read) tokens; `cached` is treated as a subset of `input` |
| `cache_write_per_mtok`   | float  | no       | falls back to `input_per_mtok` | USD per million cache-write tokens; `cache_write` is a second, disjoint subset of `input` (Anthropic bills it at 1.25x input) |

`cost_usd(input_tokens, output_tokens, cached_tokens, cache_write_tokens)` expects `output_tokens` to
already be the **billable** output figure. Gemini reports "thinking"/reasoning tokens
(`thoughtsTokenCount`) outside `candidatesTokenCount`, but Google bills them at the
output rate — that fold happens once, upstream, in `rupu-agent`'s runner
(`billable_output_tokens = output_tokens + reasoning_tokens`), so every transcript's
persisted `output_tokens` and every downstream cost call already include reasoning.
There is no separate reasoning-token parameter to this function or to the `[pricing]`
schema — passing reasoning tokens again here would double-bill them.

---

## `[storage]`

| Key                             | Type   | Default | Notes |
|----------------------------------|--------|---------|-------|
| `archived_session_retention`     | string | `30d`   | Default `rupu session prune` cutoff |
| `archived_transcript_retention`  | string | `30d`   | Default `rupu transcript prune` / `rupu cleanup` cutoff |

---

## `[runtime]`

| Key              | Type    | Default | Notes |
|------------------|---------|---------|-------|
| `max_open_files` | integer | unset   | Soft open-file limit rupu raises itself to at startup, and the cap for automatic growth when a fan-out runs near the limit. Unset: raise to 10240 (capped at the hard limit) and grow on demand up to the hard limit / macOS `kern.maxfilesperproc`. Values above that cap are clamped with a warning (raising it needs root). Overridden by `--max-open-files` / `RUPU_MAX_OPEN_FILES`. Past the cap, new agent runs are paced (delayed, logged) until running ones release descriptors, instead of failing with "Too many open files". |
| `max_concurrent_jobs` | integer | unset | Hard ceiling on how many local agent jobs run at once, enforced process-wide no matter what a workflow's `max_parallel:` or `max_concurrency:` declares — the machine-level bound that stops a wide fan-out or `split:` DAG from running the host out of memory. Unset: a RAM-derived default of `clamp(total_RAM_GB / 4, 4, 128)` (≈64 on a 256 GB host, 4 on a 16 GB laptop). Overridden by `RUPU_MAX_CONCURRENT_JOBS`. `0` means no ceiling. Does not throttle remote (`distribute:`) units — those are bounded by their own host's setting. |
| `min_free_memory_mb` | integer | unset | Memory-watchdog headroom: a new local agent job is held back until at least this many MB of system memory is available, so an in-flight allocation spike can't push the machine over. Unset: a RAM-derived default of `max(total_RAM × 0.10, 4096)`. Overridden by `RUPU_MIN_FREE_MEMORY_MB`. `0` disables the watchdog (the ceiling still applies). A held-back job proceeds anyway after 60 s if memory never recovers, and logs a run warning either way. Available memory is read from `/proc/meminfo` (Linux) or `vm_stat` (macOS); where it can't be read, the watchdog does nothing. |

---

## `[policy]`

| Key    | Type            | Default | Notes |
|--------|-----------------|---------|-------|
| `lock` | array\<string\> | `[]`    | Dotted config-key paths (e.g. `permission_mode`, `autoflow.max_active`) whose GLOBAL value overrides project + env at resolution. A customer layer's `[policy].lock` locks keys against the project layer only (see *Customer layer*). A project cannot declare its own locks. |

---

## `[recovery]`

What to do when a model's reply cannot be used or the provider fails the request. See
[response-outcomes.md](response-outcomes.md) for the recovery ladder.

| Key                    | Type  | Default | Notes |
|------------------------|-------|---------|-------|
| `fallbacks`            | array of `{ model, provider? }` | `[]` | Ordered fallback chain used by every agent that declares no `fallbacks:` of its own (an agent's list replaces this one, not merged). An entry without `provider` means the provider the run started on; entries on that provider are tried first, then other providers. Unknown keys are an error |
| `server_side_fallback` | bool  | `true`  | Ask Anthropic to fall back server-side when the requested model refuses. Applies only to API-key auth on `claude-fable-5`, `claude-fable-5-1`, `claude-opus-5`, `claude-opus-5-5` and `claude-sonnet-5-5`; never OAuth. Set `false` to stop sending it |

```toml
[recovery]
fallbacks = [
  { model = "claude-sonnet-5-5" },
  { provider = "openai-codex", model = "gpt-5.6-cyber" },
]
server_side_fallback = true
```

A cross-provider entry sends the conversation to that provider's API.

---

## `[netflow]`

What netflow records and where it is shown: [netflow.md](netflow.md).

| Key                   | Type    | Default | Notes |
|------------------------|---------|---------|-------|
| `asn_auto_refresh`     | bool    | `true`  | Keep the IP→ASN enrichment table fresh without operator action: `rupu cp serve`'s sweep tick (and the CP netflow API on first use) downloads it when it is missing or older than `asn_refresh_interval_days`. The download runs in the background, never inline on a request |
| `asn_refresh_interval_days` | integer | `7` | Age, in days, after which the ASN table counts as stale and is re-downloaded. `0` treats it as always stale |
| `asn_source_url`       | string  | `https://iptoasn.com/data/ip2asn-combined.tsv.gz` | Where the ASN table is downloaded from (a gzipped ip2asn TSV) |
| `subprocess_capture`   | bool    | `true`  | Master switch for observing the network connections an agent's `bash` commands open (passive OS socket-table observation: no proxy, no root). Where the OS backend cannot start, the run records why instead of going silent. `RUPU_NETFLOW_SUBPROCESS=0` forces it off without editing config |
| `subprocess_poll_ms`   | integer | `50`    | Linux only: interval between socket-table polls while a `bash` call runs |
| `subprocess_linger_ms` | integer | `3000`  | How long after a `bash` call's shell exits its sockets are still attributed to that call (catches connections closing just after exit); a socket still open when the linger ends is recorded as still open. Clamped to one day |
| `cp_index_budget_mb`   | integer | `256`   | Memory budget for the flow rows `rupu cp serve` keeps in its netflow index. Over budget, the rows of the files with the oldest flows are dropped and re-read from disk when a view needs them: wide windows get slower, answers never change. Per-file summaries (timestamps, counts, origins) always stay in memory. Check usage with `GET /api/netflow/index`. Takes effect on the next request after a config change made through the control plane; hand edits to `config.toml` need a restart. |

---

## `[workflow]`

Gates the `run:` workflow step kind, which executes declared commands and is
therefore opt-in.

| Key                  | Type            | Default | Notes |
|----------------------|-----------------|---------|-------|
| `run_step_enabled`   | bool            | `false` | Whether `run:` steps may execute. A workflow that reaches a `run:` step while this is off **fails** — it never silently skips the step |
| `run_step_allowlist` | array\<string\> | `[]`    | Executables a `run:` step may invoke, matched on basename (so `/bin/bash` and `bash` gate alike). Empty means any executable is allowed (when `run_step_enabled`) |

---

## `[cp]`

Runtime settings for `rupu cp serve` (the control-plane HTTP server). Absent fields
fall back to the CP's compiled defaults.

| Key                                   | Type    | Default          | Notes |
|----------------------------------------|---------|------------------|-------|
| `max_workspace_bytes`                  | integer | `268435456` (256 MiB) | Max bytes for a workspace-sync payload/delta |
| `autoflow_reconcile_enabled`           | bool    | `true`           | Runs the autoflow reconcile loop in-process |
| `autoflow_reconcile_interval_secs`     | integer | `60`             | |
| `cron_tick_enabled`                    | bool    | `true`           | Runs the cron/event-trigger tick loop in-process |
| `cron_tick_interval_secs`              | integer | `60`             | |
| `gate_sweep_enabled`                   | bool    | `true`           | Enforces gate `on_timeout` routing and reaps orphaned runs with a dead `runner_pid` |
| `gate_sweep_interval_secs`             | integer | `60`             | |

---

## `[agentiflow]`

Background supervision of agentiflow runs. A coordinator that dies (SIGKILL, a crash,
the machine going down) leaves its run `running` forever and its detached units burning
tokens; the orphan reaper records such a run as `failed` (`orphaned: coordinator pid <p>
not running`) and stops its units. Two things run it: `rupu agentiflow serve` (a
foreground loop for hosts without the control plane) and `rupu cp serve` (on its gate-sweep
tick, every `[cp].gate_sweep_interval_secs`). For what an agentiflow is and how to run
one, see [agentiflows.md](agentiflows.md).

| Key                   | Type    | Default | Notes |
|-----------------------|---------|---------|-------|
| `serve_enabled`       | bool    | `true`  | `false` makes `rupu agentiflow serve` print that it is disabled and exit |
| `serve_interval_secs` | integer | `60`    | Seconds between `rupu agentiflow serve` sweeps (it also sweeps once at startup). `cp serve` reaps on its own cadence |
| `reaper_enabled`      | bool    | `true`  | Turns the reaper off in both `rupu agentiflow serve` and `rupu cp serve`; with it off `agentiflow serve` has nothing to do and exits |

---

## `[findings]`

Limits and hints for recorded findings; see [coverage.md](coverage.md#finding-reports)
for what a finding report contains and how artifacts are stored.
`rupu findings import` reads the size limits below from the global `config.toml`
only.

| Key                  | Type            | Default                     | Notes |
|----------------------|-----------------|-----------------------------|-------|
| `artifact_max_bytes` | integer         | `524288000` (500 MiB)       | Per-file cap for copying a finding's artifacts into `<RUPU_HOME>/findings/artifacts`. Larger files are recorded by hash only (`stored: external`) |
| `artifact_max_files` | integer         | `500`                       | Most files one report's artifacts may expand to (a directory counts every file inside it). Checked before anything is copied; a larger set rejects the finding |
| `artifact_total_max_bytes` | integer   | `2147483648` (2 GiB)        | Most bytes one report's artifacts may add up to in the store: only files that are copied count, and a file over `artifact_max_bytes` (recorded by reference) does not. Checked before anything is copied; a larger set rejects the finding with the total named |
| `report_max_bytes`   | integer         | `262144` (256 KiB)          | Serialized-size budget for one `full` finding report; a larger report is rejected with the size named |
| `ticket_patterns`    | array\<string\> | `[]`                        | Patterns (regexes or URL prefixes) that identify existing tickets in your organisation; appended to the finding-writing guidance agents get under the `full` profile. Nothing organisation-specific ships in rupu |
| `export_id_prefix`   | string          | `SEC`                       | Prefix of the per-project display number on exported finding reports (`SEC-001`, `SEC-002`, …), used by `rupu findings export` and the control plane's downloads. Read from the **global** `config.toml` only: a project `.rupu/config.toml` never changes it, because it ends up in file names and document text. Must match `^[A-Za-z][A-Za-z0-9_-]{0,15}$`; anything else is ignored with a warning and `SEC` is used. See [Exporting reports](coverage.md#exporting-reports) |

---

## `[update]`

| Key       | Type   | Default   | Notes |
|-----------|--------|-----------|-------|
| `channel` | string | `stable`  | `stable` \| `beta` — which release channel `rupu update` tracks |
| `check`   | bool   | `true`    | Whether normal commands print a passive "update available" notice |

---

## Deprecated keys (accepted, inert)

These keys still parse without error — for backward compatibility with an existing
`config.toml` — but drive nothing. Delete them; a future release will reject them
outright.

| Key                        | Status | Replacement |
|-----------------------------|--------|-------------|
| `[retry]` (`max_attempts`, `initial_delay_ms`) | Never read by anything since Slice A | `[providers.<name>].max_retries` |
| `[cp].agent_authoring_ui`   | The CP web app's classic agent-authoring UI was deleted; the "next" UI is now the only UI | none needed |
| `[cp].workflow_editor_ui`   | The CP web app's classic workflow-editor UI was deleted; the "next" UI is now the only UI | none needed |

Loading a config with any of these keys present logs a `tracing::warn!` pointing at
this table; the rest of the config still loads normally.
