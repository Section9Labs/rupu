# Provider reference

Slice B-1 adds four LLM providers, each supporting two authentication modes. This document is the canonical reference for what works, how to configure it, and how to debug it. For step-by-step walkthroughs, see `docs/providers/<name>.md`.

## Provider × auth-mode matrix

| Provider           | API key | SSO  | SSO flow         | Notes                                                                                      |
| ------------------ | :-----: | :--: | ---------------- | ------------------------------------------------------------------------------------------ |
| anthropic          |   ✓     |  ✓   | Browser callback | Console API key OR Claude.ai SSO.                                                          |
| openai             |   ✓     |  ✓   | Browser callback | Platform API key OR ChatGPT SSO. Different endpoints under hood.                           |
| gemini             |   ✓     |  ✓   | Browser callback | API key via Google AI Studio (`AIzaSy…`). SSO via Vertex / CLI also supported.            |
| copilot            |   ✓     |  ✓   | Device code      | API-key path uses a GitHub PAT (`GITHUB_TOKEN`). Requires paid Copilot.                   |
| openai-compatible  |   ✓     |  —   | —                | Generic adapter for any `/v1/chat/completions` endpoint (vLLM, Oracle GenAI, Together, …). |

Anthropic remains the most exercised provider; Copilot's API-key path is most reliable for users who already have `gh auth login` configured.

## Accounts vs. vendor kind

A provider *name* used to mean two things at once: which credential to use, and which vendor client to build. They're now split:

- **Account** — the identity. Whatever you pass to `--account` (below) is a credential slot: `anthropic`, `anthropic-work`, `gh-personal`, ... Freeform, and it's what an agent's `provider:` frontmatter field names.
- **`kind`** — the vendor. Selects which client authenticates the account: `anthropic`, `openai`, `gemini`, `copilot`, `github`, `gitlab`, `linear`, `jira`, or `openai-compatible` (see below).

A bare vendor name used as the account (`--account anthropic`) needs no `--kind` — the account name *is* the vendor, exactly like before this existed, and every example below that uses a plain provider name still works unchanged. To hold a second account of the same vendor — a work identity and a personal identity, each with an independent credential including an independent SSO token — give it a distinct name and its vendor via `--kind`:

```sh
rupu auth login --account anthropic-work     --kind anthropic --mode sso
rupu auth login --account anthropic-personal --kind anthropic --mode sso
```

The first time a new account name is used with `--kind`, rupu writes `[providers.<account>] kind = "<kind>"` into `~/.rupu/config.toml` so every later resolution (agent frontmatter, `rupu auth status`, credential refresh) knows that account's vendor. `--kind` is only required the first time an account is created — after that it's inferred from the config entry. Point an agent at a specific account the same way you'd point it at any provider: `provider: anthropic-work` in the frontmatter.

**Exception:** a `github` or `gitlab` account (e.g. `--account gh-work --kind github`) writes `[scm.<account>] kind = "<kind>"` instead — SCM accounts route repos/issues through `rupu-scm`'s account-selection rules, which only ever look at `[scm.*]`. Every other vendor, this document's examples included, lands in `[providers.*]` as described above. See `docs/scm.md`'s "Multi-account routing" section for the SCM side: rules, precedence, the ambiguity error, `rupu scm bind`, and `rupu scm accounts`.

`--account` accepts `--provider` as an alias throughout this document's examples, for scripts and muscle memory from before this feature existed.

### Per-account API key env var

`RUPU_<UPPER_ACCOUNT>_API_KEY` is read as a fallback when no stored credential exists for that account (unless the caller explicitly asked for SSO — see "Refresh" below; SSO never silently falls back to an API key). Non-alphanumeric characters in the account name map to `_`, so `anthropic-work`, `anthropic_work`, and `anthropic.work` all read the *same* variable, `RUPU_ANTHROPIC_WORK_API_KEY` — distinct accounts whose names only differ by punctuation collide on this fallback. Pick account names that stay distinct once mangled if you rely on it for more than one of them.

## Auth flows

### API key

```sh
rupu auth login --account <name> --mode api-key --key <secret>
# or omit --key to read from stdin
echo -n "$KEY" | rupu auth login --account <name> --mode api-key
# a second account of the same vendor needs --kind the first time:
rupu auth login --account <name> --kind <vendor> --mode api-key --key <secret>
```

Stored in the OS keychain at `rupu/<provider>/api-key`.

### SSO browser callback (Anthropic, OpenAI, Gemini)

```sh
rupu auth login --account <name> --mode sso
```

Steps:
1. rupu binds a localhost listener on a free port (`127.0.0.1:0`).
2. A browser opens to the provider's authorize URL with PKCE challenge.
3. Complete login in the browser; the page redirects to `http://127.0.0.1:<port>/callback`.
4. rupu validates the redirect's `state` (CSRF protection), exchanges the auth code for tokens, and stores them in the keychain at `rupu/<provider>/sso`.
5. The browser shows "Authentication complete — return to your terminal."

**Headless (Linux without `DISPLAY`/`BROWSER`):** the browser-callback flow errors out with a message pointing at `--mode api-key`. There's no headless fallback for these three providers.

### SSO device code (Copilot)

```sh
rupu auth login --account copilot --mode sso
```

Steps:
1. rupu requests a device code from `github.com/login/device/code`.
2. rupu prints `Visit https://github.com/login/device and enter code: ABCD-1234`.
3. Open the URL in any browser, paste the code, authorize the rupu OAuth app.
4. rupu polls `github.com/login/oauth/access_token` until the user grants access.
5. The GitHub token is exchanged for a Copilot API token; both are stored at `rupu/copilot/sso`.

### Default precedence

When an agent file declares `provider: anthropic` without an explicit `auth:` field, the credential resolver applies this order:
1. SSO entry if present and not expired beyond refresh.
2. API-key entry if present.
3. Error: `no credentials configured for <account>. Run: rupu auth login --account <account> --mode <api-key|sso>`.

To force a specific mode, set `auth: api-key` or `auth: sso` in the agent's YAML frontmatter.

### Refresh

SSO access tokens expire (typically 1 hour). The resolver pre-emptively refreshes when `expires_at - now < 60s` on a `get()` call, using the stored refresh token. On refresh failure: an actionable error naming the account and the mode that needs re-authenticating (`refresh failed for '<account>': HTTP <code>. Re-authenticate this account (mode: sso) to continue.`). There is no automatic fall-back to API-key — the user explicitly chose SSO.

### Logout

```sh
rupu auth logout --account <name>             # both api-key and sso
rupu auth logout --account <name> --mode sso  # just one
rupu auth logout --all                         # all credentials (with confirmation)
rupu auth logout --all --yes                   # skip confirmation
```

## Configuration (`~/.rupu/config.toml`)

```toml
[providers.anthropic]
# All fields optional; vendor defaults apply when absent.
base_url = "https://custom-proxy.example.com"
timeout_ms = 60000
max_retries = 5
max_concurrency = 4
default_model = "claude-sonnet-4-6"

[providers.openai]
org_id = "org-abc123"
default_model = "gpt-5"

[providers.gemini]
default_model = "gemini-2.5-pro"

[providers.copilot]
# typically nothing needed

[[providers.openai.models]]   # custom/private models
id = "gpt-5-internal-finetune"
context_window = 200000
max_output = 16000
```

### Field reference

- **`base_url`** (`Option<String>`): override the vendor's default API endpoint. Useful for proxies and Azure-OpenAI-style deployments. Default: vendor's documented URL.
- **`org_id`** (`Option<String>`, OpenAI): organization scope for billed usage. Sent as the `OpenAI-Organization` header on the platform API (`api.openai.com`). Not sent on the ChatGPT-subscription endpoint, which is scoped by account rather than organization. Ignored by every other provider.
- **`region`** (`Option<String>`): **accepted but not currently used.** It is reserved for a Vertex AI regional endpoint (e.g. `us-central1`), and no shipped Gemini client targets one — rupu's Gemini paths are AI Studio, Gemini CLI, and Antigravity, none of which is region-scoped. Setting it changes nothing today.
- **`timeout_ms`** (`Option<u64>`): per-request *inactivity* deadline, applied as the HTTP client's connect + read timeout. A long generation that keeps streaming is never cut off; a connection that goes silent for this long is aborted. Default: `120000` (2 min). `0` is treated as unset.
- **`max_retries`** (`Option<u32>`): retries *after* the first attempt on a retryable error (`RateLimited`, `Transient`, 5xx, 429/529, transport failures). Permanent errors — 4xx, auth failures, malformed requests — are never retried, and a stream that already emitted output is never re-issued. Backoff is 2s, doubling, capped at 60s. Default: `1`. (This doc previously claimed `5`; the implementation's real budget was `1`, chosen so `ProviderRouter` can fail over to another vendor quickly instead of spending ~30s of backoff on one. The doc was corrected to match the code — set the key explicitly if you want a larger budget.)
- **`max_concurrency`** (`Option<usize>`): per-provider semaphore size — the maximum number of in-flight LLM calls to this provider in one rupu process. Defaults: anthropic 4, openai 8, gemini 4, copilot 4. The semaphore is created once per process on first use, so changing this mid-process has no effect. `0` is treated as unset.
- **`default_model`** (`Option<String>`): model used when an agent file omits `model:`. No global default — the agent must either set `model:` or have one resolvable here.
- **`[[providers.<name>.models]]`** (`Vec<CustomModel>`): register private/internal/fine-tuned models that aren't returned by `/v1/models`. Each entry takes `id` (required) plus optional `context_window` and `max_output`.

## OpenAI-compatible providers (Oracle GenAI, vLLM, …)

`kind` is not exclusive to this section — it accepts any built-in vendor
name (`anthropic`, `openai`, `gemini`, `copilot`, `local`, `github`,
`gitlab`, `linear`, `jira`, plus the aliases `openai_codex`, `codex`,
`google_gemini`, `github_copilot`) and is the mechanism for declaring a second account of
a vendor you already use under its bare name; see "Accounts vs. vendor
kind" above. `kind = "openai-compatible"` is the one value that selects
a different, generic client instead of a specific vendor: it connects
rupu to any server that speaks the `/v1/chat/completions` API with a
static Bearer key — self-hosted vLLM, Oracle GenAI, Together, Fireworks,
OpenRouter, and similar endpoints. It's also the only kind that
*requires* `base_url` and `default_model` (enforced at config load —
every other kind infers its endpoint from the vendor).

### Config (`~/.rupu/config.toml`)

```toml
default_provider = "oracle"

[providers.oracle]
kind = "openai-compatible"
base_url = "http://192.29.35.246:8080"
default_model = "/raid/models/zai-org/GLM-5.2-FP8"
stream = true   # set false if the server has no SSE endpoint

  [[providers.oracle.models]]
  id = "/raid/models/zai-org/GLM-5.2-FP8"
  context_window = 131072
  max_output = 8192
```

`base_url` may include or omit a trailing `/v1` — rupu normalises both.
Each `[[providers.<name>.models]]` entry requires `id`; `context_window`
and `max_output` are optional (defaults 32768 / 8192 when omitted). These
surface in `rupu models list --provider oracle`.

### Authentication

Only API-key auth is supported for openai-compatible providers — there is
no SSO flow.

```sh
# Store the Bearer key (written to auth.json, mode 0600). The account
# already has `kind = "openai-compatible"` declared in config above, so
# --kind isn't needed here:
rupu auth login --account oracle --mode api-key   # prompts for the key
# …or pipe from stdin / paste inline:
echo -n "$KEY" | rupu auth login --account oracle --mode api-key
```

For CI / ephemeral environments, set the env var instead (rupu reads it
automatically and does not require a prior `rupu auth login`):

```sh
export RUPU_ORACLE_API_KEY=sk-...
```

The env var name is always `RUPU_<UPPERCASED_ACCOUNT_NAME>_API_KEY`, with
non-alphanumeric characters in the account name mapped to `_` — see the
collision caveat under "Per-account API key env var" above.

### Running an agent

Set `provider: oracle` in the agent's YAML frontmatter:

```markdown
---
name: oracle-codereview
provider: oracle
model: /raid/models/zai-org/GLM-5.2-FP8
---

You review code changes for correctness and style.
```

Then run:

```sh
rupu run oracle-codereview
```

### Workflow steps and subagents

Workflow steps and dispatched subagents support `openai-compatible` providers
the same way `rupu run` does: a step whose agent sets `provider: oracle`
resolves the `[providers.oracle]` config entry and builds the same client.

### Per-provider walkthrough

See `docs/providers/openai-compatible.md` for a step-by-step setup guide.

## Model resolution

`rupu run <agent>` resolves the agent's `model:` field through three sources in order:
1. **Custom** — `[[providers.<name>.models]]` entries from `~/.rupu/config.toml`.
2. **Live cache** — `~/.rupu/cache/models/<provider>.json` (TTL 1h). Populated by `rupu models refresh` or lazily on first `rupu models list`.
3. **Baked-in** — Copilot and Gemini ship a curated v0 list since their public listing endpoints are limited.

```sh
rupu models list              # built-in vendors + every declared account
rupu models list --provider openai
rupu models refresh           # re-fetch live caches
rupu models refresh --provider anthropic
```

`--provider` accepts a **declared account name** as well as a built-in vendor
name — `rupu models refresh --provider anthropic-work` refreshes that account
against its declared vendor (`[providers.anthropic-work] kind = "anthropic"`),
using that account's own credential, and caches the result under the account
name so two accounts of one vendor keep separate model lists. A name that is
neither a built-in vendor nor a declared account is an error with a non-zero
exit, not a silent no-op.

An `openai-compatible` account has no live listing endpoint — its models are
whatever `[[providers.<name>.models]]` declares — so `models refresh` reports
that rather than pretending to fetch.

If the agent's `model:` value isn't found in any source, rupu errors with:
> `model 'xyz' not found for provider 'openai'. Run 'rupu models list --provider openai' to see available models, or add a custom entry to ~/.rupu/config.toml.`

## Troubleshooting

**`rupu auth status` shows `✓` but `rupu run` errors with Unauthorized.**
The token may have expired faster than the refresh window expected. Re-login with `rupu auth login --account <name> --mode sso`. If api-key, the key was rotated server-side — generate a new one and re-login.

**SSO login fails on a server / over SSH.**
The browser-callback flow can't reach a desktop. Use `--mode api-key`. Copilot's device-code SSO is the only flow that works headless — visit the URL from any browser anywhere and the polling completes.

**Custom model rejected.**
Add the model under `[[providers.<name>.models]]` in `~/.rupu/config.toml` with at least the `id` field, then retry. Custom entries always take precedence over live and baked-in.

**Gemini API-key login fails.**
Plan 1 didn't wire the AI Studio API-key endpoint (the lifted client only supports Vertex/CLI OAuth). Use `--mode sso` for now, or track the deferred work in `TODO.md`.

**Cargo build prompts for keychain access on every `cargo run`.**
macOS treats each freshly-built binary as a different code identity. Track the deferred signing/notarization work in `TODO.md`. Quick fix: click "Always Allow" once on the first prompt — the trust persists per binary path until the next rebuild.

**`rupu auth logout --all` removes credentials I didn't expect.**
By design — `--all` iterates every stored account × mode. Use `--account <name>` (with optional `--mode <m>`) for surgical removals.

## Deferred / future

- Richer usage visualization / dashboards beyond `rupu usage` — per-response usage is captured in JSONL transcripts and joined with workflow-run metadata today; future work is higher-level visualization, not the base reporting command.
- Local-model provider (Ollama / llama.cpp) — out of scope for Slice B-1; planned for a later slice.
- Cost accuracy enhancements — `rupu usage` reports USD from built-in/default pricing tables today; users with strict accounting needs should override provider/model pricing in config.
- Cross-provider model aliases (e.g., `model: smart`) — not planned; explicit model names are clearer.
- Vendor-specific model features (Anthropic prompt-cache toggles, OpenAI structured-output mode, Gemini grounding) — adapters expose them as opaque pass-through fields where natural; no first-class rupu surface yet.
