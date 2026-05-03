# OpenAI

## API key vs ChatGPT SSO

OpenAI's two paths use different endpoints under the hood. Both work in rupu; pick based on what you have:

- **Platform API key** (`platform.openai.com`) — for paid API tier users. Targets `api.openai.com/v1/responses`. Charged via your OpenAI billing.
- **ChatGPT SSO** (browser callback) — for ChatGPT Plus/Pro subscribers. Targets `chatgpt.com/backend-api/codex/responses`. Usage is included in your subscription rather than billed separately.

## Get an API key

1. Sign in to [platform.openai.com](https://platform.openai.com).
2. API keys → Create new secret key.
3. Copy the `sk-...` value.

```sh
rupu auth login --provider openai --mode api-key --key sk-XXX
```

If your account belongs to an organization, set `org_id` in `~/.rupu/config.toml`:

```toml
[providers.openai]
org_id = "org-abc123"
```

## ChatGPT SSO

```sh
rupu auth login --provider openai --mode sso
```

A browser opens to `auth.openai.com/oauth/authorize`. Sign in with your ChatGPT account. The redirect lands on rupu's localhost listener; the credential is stored at `rupu/openai/sso`.

The token includes a `chatgpt_account_id` JWT claim that tells the client to use the ChatGPT backend URL instead of the platform API. This is automatic — rupu detects the difference and routes accordingly.

## Example agent file

```markdown
---
name: codegen-openai
description: Generate code via OpenAI Responses API.
provider: openai
auth: api-key
model: gpt-5
---

You generate complete, idiomatic code given a description. No commentary.
```

For SSO use `auth: sso`.

## Configuration knobs

```toml
[providers.openai]
org_id = "org-abc123"               # required for org-scoped keys
base_url = "https://api.openai.com"  # rarely overridden
default_model = "gpt-5"
timeout_ms = 120000
max_retries = 5
max_concurrency = 8                  # OpenAI's per-key limits are higher than Anthropic's
```

Custom models (org-private fine-tunes):

```toml
[[providers.openai.models]]
id = "ft:gpt-4o-2024-05-13:my-org::abc123"
context_window = 128000
max_output = 4096
```

## Known quirks

- **Responses API only.** rupu uses OpenAI's newer `/v1/responses` (the same one Codex uses), not the older `/v1/chat/completions`. Any `gpt-3.5-*` or `text-davinci-*` models that pre-date Responses won't work.
- **Account ID detection.** ChatGPT-issued OAuth tokens carry an account_id; if a token is malformed and the account_id can't be extracted, the client falls back to the platform URL and you'll see auth failures. Re-login fixes it.
