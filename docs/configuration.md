# Configuration Reference

> See also: [providers.md](providers.md) · [scm.md](scm.md) · [using-rupu.md](using-rupu.md)

Complete reference for `~/.rupu/config.toml` (global) and `<repo>/.rupu/config.toml`
(project-local override). This page enumerates every key `rupu-config` accepts; for
narrative walkthroughs of the provider and SCM sections, see the linked docs above.

---

## Locations and layering

- `~/.rupu/config.toml` — global config.
- `<project>/.rupu/config.toml` — project-local overrides. Scalars and tables override the
  global value; arrays replace rather than merge.
- Every section is `deny_unknown_fields`: an unrecognized key fails to parse. Three
  keys are accepted-but-inert exceptions to this — see **Deprecated keys** below — kept
  as opaque no-op shims so an old config doesn't lose every other setting it carries.
- All fields are optional. A missing value at one layer can be supplied by another;
  the actual defaults applied are documented per key below.

---

## Top-level keys

| Key               | Type   | Default             | Notes                                                        |
|-------------------|--------|----------------------|---------------------------------------------------------------|
| `default_provider` | string | none                 | Provider used when an agent file omits `provider:`            |
| `default_model`     | string | `claude-sonnet-4-6`  | Model used when an agent file omits `model:` and `[providers.<name>].default_model` is also unset |
| `permission_mode`   | string | `ask`                | Fallback permission mode when neither the agent nor `--mode` sets one |
| `log_level`         | string | none (info-ish default via `RUPU_LOG`) | Logging verbosity                        |

---

## `[bash]`

| Key             | Type            | Default                          | Notes |
|-----------------|-----------------|-----------------------------------|-------|
| `timeout_secs`  | integer         | `120`                             | Timeout for a single `bash` tool invocation |
| `env_allowlist` | array\<string\> | `[]`                              | Extra env vars forwarded into the bash subprocess, beyond the always-allowed `PATH`/`HOME`/`USER`/`TERM`/`LANG` |

---

## `[providers.<name>]`

One table per provider name (`anthropic`, `openai`, `gemini`, `copilot`, or a
user-declared `openai-compatible` name such as `oracle`). Full narrative reference:
[providers.md](providers.md#field-reference).

| Key               | Type                     | Default                                             |
|-------------------|--------------------------|-------------------------------------------------------|
| `base_url`        | string                   | vendor's documented URL                                |
| `kind`            | string                   | none (built-in provider); `"openai-compatible"` declares a generic adapter |
| `stream`          | bool                     | `true`                                                 |
| `org_id`          | string                   | none (OpenAI-only; sent as `OpenAI-Organization`)      |
| `region`          | string                   | none (accepted but not currently used by any shipped client) |
| `timeout_ms`      | integer                  | `120000` (2 min); `0` treated as unset                 |
| `max_retries`     | integer                  | `1` (retries after the first attempt on a retryable error) |
| `max_concurrency` | integer                  | anthropic `4`, openai `8`, gemini `4`, copilot `4`; `0` treated as unset |
| `default_model`   | string                   | none — the agent must set `model:` or rely on this      |
| `models`          | array\<table\>           | `[]` — each entry: `id` (required), `context_window` (default `32768`), `max_output` (default `8192`) |

An `openai-compatible` entry additionally requires `base_url` and `default_model`
(validated at load time), and may not reuse a reserved built-in provider name
(`anthropic`, `openai`, `gemini`, `copilot`, `local`, `github`, `gitlab`, `linear`,
`jira`).

---

## `[scm]` / `[issues]`

Full narrative reference: [scm.md](scm.md#configuration).

| Key                  | Type   | Default | Notes |
|----------------------|--------|---------|-------|
| `[scm.default]`      | table  | none    | `platform`, `owner`, `repo` — fallback repo when a tool call omits `platform?` |
| `[issues.default]`   | table  | none    | `tracker`, `project` — fallback tracker when a tool call omits `tracker?` |
| `[scm.<platform>]`   | table  | none    | Per-platform override for `github` / `gitlab` (`base_url`, `timeout_ms` default `30000`, `max_concurrency` default github `8` / gitlab `6`, `clone_protocol` default `https`) |

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
| `poll_sources`        | array\<string \| table\> | `[]` | Repo (`github:owner/repo`, `gitlab:group/project`) or tracker-native (`linear:<team-id>`, `jira:<site>/<project>`) sources. A table entry adds `poll_interval` (e.g. `5m`) |
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

```toml
[pricing.anthropic."claude-sonnet-4-6"]
input_per_mtok = 3.0
output_per_mtok = 15.0
cached_input_per_mtok = 0.30   # optional; omit to bill cached tokens at the full input rate

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
| `cached_input_per_mtok`  | float  | no       | falls back to `input_per_mtok` | USD per million cached-input tokens; `cached` is treated as a subset of `input` |

`cost_usd(input_tokens, output_tokens, cached_tokens)` expects `output_tokens` to
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

## `[policy]`

| Key    | Type            | Default | Notes |
|--------|-----------------|---------|-------|
| `lock` | array\<string\> | `[]`    | Dotted config-key paths (e.g. `permission_mode`, `autoflow.max_active`) whose GLOBAL value overrides project + env at resolution. Only read from the global layer — a project cannot declare its own locks |

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
