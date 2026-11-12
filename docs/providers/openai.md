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

A browser opens to `auth.openai.com/oauth/authorize`. Sign in with your ChatGPT account. The redirect lands on rupu's localhost listener; the credential is stored in `~/.rupu/auth.json` (mode 0600).

The token includes a `chatgpt_account_id` JWT claim that tells the client to use the ChatGPT backend URL instead of the platform API. This is automatic — rupu detects the difference and routes accordingly.

## Device login

On a machine with no browser (a server, a VM, an SSH session):

```sh
rupu auth login --account openai --mode device
```

rupu prints `Visit https://auth.openai.com/codex/device and enter code: ABCD-1234`. Open that page in a browser on any machine, sign in with your ChatGPT account and enter the code; rupu stores the same credential as `--mode sso`, in the account's `sso` entry. This is Codex CLI's `--device-auth` login. The code expires in 15 minutes; only enter a code from a login you started yourself.

Device login has to be enabled for the account first: ChatGPT settings → Security (a workspace admin enables it under workspace permissions). Until it is, rupu reports `device login is not enabled for this ChatGPT account`.

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

## OpenAI Daybreak cyber access

OpenAI's [Daybreak](https://developers.openai.com/api/docs/guides/daybreak) program gates its cyber-capable models (`gpt-5.6-cyber`, for example) behind an *access program* the caller selects per request. rupu selects it with `cyber`, which it sends on the Responses API request body as `access_programs: { cyber: <value> }`:

- `standard`: the ordinary access tier (no cyber program).
- `daybreak_blue`: the Daybreak defensive program.
- `daybreak_red`: the Daybreak offensive program.

Setting `cyber` selects a program; it does not grant access. Your organization and project must already be approved for it, and the value must match the model. rupu passes the value through unchanged, so a wrong pairing comes back as OpenAI's `invalid_access_program` error on the request rather than a rupu error.

**Which logins carry it.** Both. The request body is built the same way for the platform API (`api.openai.com/v1/responses`, API key) and the ChatGPT endpoint (`chatgpt.com/backend-api/codex/responses`, SSO or device login), so the program rides on either; whether OpenAI honours it is the approval of the account the request authenticates as. Only the OpenAI client sends it: an agent on `provider: openai` (or an account declared with `kind = "openai"`, `"openai_codex"` or `"codex"`, all the same client). Every other provider ignores the value.

**Organization and project.** With an API key, `[providers.openai] org_id` is sent as the `OpenAI-Organization` header on the platform API only (never on the ChatGPT endpoint, which is scoped by the account). rupu sends no `OpenAI-Project` header, so the project is the one the API key belongs to: create the key inside the project OpenAI approved for the program. With a ChatGPT login there is no organization or project header at all; the approval is the ChatGPT account's.

**Where to set it, highest first.**

1. `rupu run <agent> --cyber <program>` overrides everything for that run. `rupu run --continue <id>` restores it with the rest of the original command; a `--cyber` given on the continue replaces it.
2. The agent's `cyber:` frontmatter ([agent-format.md](../agent-format.md#cyber)). An unknown value there is a parse error: the agent does not load.
3. A `cyber` on the `[[providers.openai.models]]` entry whose `id` is the run's model ([configuration.md](../configuration.md#cyber-access-programs)). An unknown value there is ignored with a warning in the log, not an error.

```yaml
---
name: exploit-verifier
provider: openai
model: gpt-5.6-cyber
cyber: daybreak_red
---
```

```sh
rupu run exploit-verifier --cyber daybreak_red
```

**Behaviour notes.**

- A fallback hop keeps the program only while the model stays the same; a hop to another model or provider drops it, since the right value is model-specific. A chain entry that names another model therefore runs without a program.
- The compaction summariser call never carries it: only the turn requests do.
- A session freezes the program from the agent at `rupu session start` and sends it on every turn; a session settings override of the provider or model clears it (the override's model may not take the same program). `rupu session start` has no `--cyber` flag.
- `--cyber` is not sent to remote placed units (`host:` / `distribute:`): the coordinator's flag stays on the coordinator. A placed unit runs the agent file installed on that host, so put `cyber:` in that file (or in that host's config) to run it under a program.
- `provider: openai-codex` (with a hyphen) is only the display id rupu uses in transcripts and pricing, not a built-in provider name; it works only as a declared account, `[providers.openai-codex] kind = "openai_codex"`, logged in under that name.

## Known quirks

- **Responses API only.** rupu uses OpenAI's newer `/v1/responses` (the same one Codex uses), not the older `/v1/chat/completions`. Any `gpt-3.5-*` or `text-davinci-*` models that pre-date Responses won't work.
- **Account ID detection.** ChatGPT-issued OAuth tokens carry an account_id; if a token is malformed and the account_id can't be extracted, the client falls back to the platform URL and you'll see auth failures. Re-login fixes it.
