# Anthropic

## Get an API key

1. Sign in to [console.anthropic.com](https://console.anthropic.com).
2. Settings → API Keys → Create Key.
3. The key starts with `sk-ant-`. Copy it now — the console shows it only once.

```sh
rupu auth login --provider anthropic --mode api-key --key sk-ant-XXX
rupu auth status
# anthropic   ✓        -
```

## SSO via Claude.ai

```sh
rupu auth login --provider anthropic --mode sso
```

A browser opens to `claude.ai/oauth/authorize`. Sign in with your Claude account; the page redirects to a localhost URL that rupu intercepts. The browser shows "Authentication complete" and rupu writes the credential to `rupu/anthropic/sso` in the OS keychain.

The access token expires in ~1 hour. rupu auto-refreshes ~60 seconds before expiry using the stored refresh token. Refresh failures surface a clear error pointing at this same command.

## Example agent files

`~/.rupu/agents/explain.md` — API-key auth:

```markdown
---
name: explain
description: Explain a piece of code in plain language.
provider: anthropic
auth: api-key
model: claude-sonnet-4-6
---

You explain code clearly. Be concise.
```

`~/.rupu/agents/refactor.md` — SSO auth (Claude Pro subscriber):

```markdown
---
name: refactor
provider: anthropic
auth: sso
model: claude-sonnet-4-6
---

You suggest minimal-diff refactors.
```

`~/.rupu/agents/quick.md` — no `auth:` field uses default precedence (SSO > API-key):

```markdown
---
name: quick
provider: anthropic
model: claude-sonnet-4-6
---

You answer concisely.
```

## Configuration knobs

```toml
[providers.anthropic]
base_url = "https://your-bedrock-proxy.example.com"  # rare; default is api.anthropic.com
timeout_ms = 60000
max_retries = 5
max_concurrency = 4
default_model = "claude-sonnet-4-6"
```

## Known quirks

- **Prompt caching:** Anthropic returns `cache_read_input_tokens` in the usage block. rupu maps this to `Usage.cached_tokens` and surfaces it in `Event::Usage` JSONL events. The CLI footer doesn't show it as a separate line in Plan 1; check the transcript directly.
- **Beta features:** Anthropic's "tool use" and "computer use" are stable; experimental features like extended thinking aren't yet plumbed end-to-end.
