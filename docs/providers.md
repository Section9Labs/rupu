# Provider reference

Slice B-1 adds four LLM providers, each supporting two authentication modes. This document is the canonical reference for what works, how to configure it, and how to debug it. For step-by-step walkthroughs, see `docs/providers/<name>.md`.

## Provider × auth-mode matrix

| Provider           | API key | SSO  | SSO flow         | Notes                                                                                      |
| ------------------ | :-----: | :--: | ---------------- | ------------------------------------------------------------------------------------------ |
| anthropic          |   ✓     |  ✓   | Browser callback | Console API key OR Claude.ai SSO.                                                          |
| openai             |   ✓     |  ✓   | Browser callback | Platform API key OR ChatGPT SSO. Different endpoints under hood.                           |
| gemini             |   ✓     |  ✓   | Browser callback | API key via Google AI Studio (`AIzaSy…`). SSO is the Gemini CLI / Antigravity login (Cloud Code Assist, not Vertex). |
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

Stored in `~/.rupu/auth.json` (mode 0600), the only credential store: `$RUPU_HOME/auth.json` when `RUPU_HOME` is set, or the path in `RUPU_AUTH_FILE`. rupu no longer uses the OS keychain.

### SSO browser callback (Anthropic, OpenAI, Gemini)

```sh
rupu auth login --account <name> --mode sso
```

Steps:
1. rupu binds a localhost listener on a free port (`127.0.0.1:0`).
2. A browser opens to the provider's authorize URL with PKCE challenge.
3. Complete login in the browser; the page redirects to `http://127.0.0.1:<port>/callback`.
4. rupu validates the redirect's `state` (CSRF protection), exchanges the auth code for tokens, and stores them in `~/.rupu/auth.json`.
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
5. The GitHub token is exchanged for a Copilot API token; both are stored in `~/.rupu/auth.json`.

### Default precedence

When an agent file declares `provider: anthropic` without an explicit `auth:` field, the credential resolver applies this order:
1. SSO entry if present and not expired beyond refresh.
2. API-key entry if present.
3. Error: `no credentials configured for <account>. Run: rupu auth login --account <account> --mode <api-key|sso>`.

To force a specific mode, set `auth: api-key` or `auth: sso` in the agent's YAML frontmatter.

### Refresh

SSO access tokens expire (typically 1 hour). The resolver pre-emptively refreshes when `expires_at - now < 60s` on a `get()` call, using the stored refresh token. On refresh failure: an actionable error naming the account and the mode that needs re-authenticating (`refresh failed for '<account>': HTTP <code>. Re-authenticate this account (mode: sso) to continue.`). There is no automatic fall-back to API-key — the user explicitly chose SSO.

### Credential refresh and locking

Every credential lives in one file, `~/.rupu/auth.json`, and rupu keeps a lock file beside it, `auth.json.lock`. `rupu auth login`, `rupu auth logout` and every SSO token refresh hold that lock while they change the file, so several rupu processes (a `cp serve`, a workflow, a second terminal) can share one login safely.

- **One refresh at a time.** A process that finds an SSO token near expiry takes the lock, re-reads the stored credential, and asks the provider for a new token only if one is still needed (a refresh forced by a 401 always asks, from the latest stored token); if another process already refreshed it, this one uses that result. New tokens are written to a temporary file and renamed into place, so a reader never sees a half-written file. A refresh the provider doesn't answer gives up after 30 seconds and releases the lock.
- **A stuck lock is reported, not waited on forever.** Waiting for the lock is bounded at 30 seconds. After that the command fails with `credential store is locked by another process (<path>/auth.json.lock); retry, or find and stop the process holding it`, and writes nothing. A stopped process or a hung network-filesystem lock daemon can cause it.
- **A token that was refreshed but couldn't be saved is not lost.** Providers rotate the refresh token when they issue a new access token. If the write then fails (a read-only directory, a full disk), the process keeps using the new credential, holds it in memory, and prints to stderr:

  ```
  rupu: the '<account>' token was refreshed but could not be written to <path>: <error>. This process keeps using the new token and retries the write on its next write of the credential file or refresh of this account; if the next rupu command cannot authenticate, run: rupu auth login --account <account> --mode sso
  ```

  Until a write succeeds the file still holds the old, now dead, refresh token, so fix the cause and run again, or log in again as the message says. Nothing retries on its own: the retry happens the next time that process writes the file or refreshes that account. If another process logs the account in or out, or refreshes it, in the meantime, the in-memory copy is discarded and the file wins.
- **Exit waits for writes in flight.** Before it exits, rupu waits up to 10 seconds for a token write still in progress (`rupu: waiting for N credential write(s) to finish…`, then `rupu: gave up waiting for credential writes after 10s` if it doesn't land).
- **SIGTERM.** A rupu process that receives SIGTERM (a cancelled run, `systemctl stop`) starts no new model call, summariser call or tool, so a run stops as aborted or cancelled at its next step instead of finishing. It waits at most 10 seconds for a credential write already in flight (not at all when there is none), then dies by the signal, so a shell or supervisor sees a signal death rather than an exit code. `rupu autoflow serve` does its own graceful SIGTERM shutdown and then waits for writes as it exits.

SIGKILL, or a refresh still running after the 10-second wait, can still lose a rotated token; `rupu auth login` recovers.

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
prompt_cache = true   # default; set false for gateways that reject cache_control

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

- **`base_url`** (`Option<String>`): override the vendor's default API endpoint. Useful for proxies and Azure-OpenAI-style deployments. Default: vendor's documented URL. **Today only `kind = "openai-compatible"` accounts read it** — an anthropic-kind account's `base_url` is ignored and the client always targets Anthropic's own API (tracked as [I-92](../ISSUES.md)). The only way to route Anthropic traffic to a gateway today is the process-wide `RUPU_ANTHROPIC_BASE_URL_OVERRIDE` environment variable (see `prompt_cache` below).
- **`org_id`** (`Option<String>`, OpenAI): organization scope for billed usage. Sent as the `OpenAI-Organization` header on the platform API (`api.openai.com`). Not sent on the ChatGPT-subscription endpoint, which is scoped by account rather than organization. Ignored by every other provider.
- **`region`** (`Option<String>`): **accepted but not currently used.** It is reserved for a Vertex AI regional endpoint (e.g. `us-central1`), and no shipped Gemini client targets one — rupu's Gemini paths are AI Studio, Gemini CLI, and Antigravity, none of which is region-scoped. Setting it changes nothing today.
- **`timeout_ms`** (`Option<u64>`): per-request *inactivity* deadline, applied as the HTTP client's connect + read timeout. A long generation that keeps streaming is never cut off; a connection that goes silent for this long is aborted. Default: `120000` (2 min). `0` is treated as unset.
- **`max_retries`** (`Option<u32>`): retries *after* the first attempt on a retryable error (`RateLimited`, `Transient`, 5xx, 429/529, transport failures). Permanent errors — 4xx, auth failures, malformed requests — are never retried, and a stream that already emitted output is never re-issued. Backoff is 2s, doubling, capped at 60s. Default: `1`. (This doc previously claimed `5`; the implementation's real budget was `1`, chosen so `ProviderRouter` can fail over to another vendor quickly instead of spending ~30s of backoff on one. The doc was corrected to match the code — set the key explicitly if you want a larger budget.)
- **`max_concurrency`** (`Option<usize>`): per-provider semaphore size — the maximum number of in-flight LLM calls to this provider in one rupu process. Defaults: anthropic 4, openai 8, gemini 4, copilot 4. The semaphore is created once per process on first use, so changing this mid-process has no effect. `0` is treated as unset.
- **`prompt_cache`** (`Option<bool>`, Anthropic): explicit prompt caching. On by default — unset means on. Cache reads and writes appear in usage as `cached_tokens` / `cache_write_tokens` and are priced at the model's cache-read / cache-write rates. Ignored by every other provider.

  *Placement.* Every Anthropic request carries at most two `cache_control: {"type": "ephemeral"}` breakpoints (5-minute TTL). One sits on the last system block, which caches the tool definitions and system prompt together (on the last tool definition when there is no system prompt). The other sits on the last content block of the final message and moves forward each turn, so the prior conversation is re-read from the cache. When the final message has nothing that can carry a marker — for example the empty `tool_result` a silent `bash` command returns — the marker walks back to the last eligible block of the nearest earlier message (typically the preceding `tool_use`), at most three messages back. Thinking blocks, empty text blocks, and empty tool results are never marked, and prefixes shorter than the model's minimum cacheable length (512–4096 tokens) are silently not cached. rupu's internal compaction-summary request opts out of caching on its own, since nothing ever re-reads its prefix.

  *Opting out.* Set `prompt_cache = false` on an account to send no breakpoints at all. The setting is per account, so `[providers.anthropic]` keeps caching while another account turns it off. An agent's `anthropicPromptCache:` frontmatter overrides this key in either direction (see [agent-format.md](agent-format.md#anthropicpromptcache)).

  *Gateways and proxies.* An **Anthropic-compatible gateway or proxy that rejects `cache_control`** fails every request until caching is turned off. Note that an anthropic-kind account's `base_url` is **not yet honored** ([I-92](../ISSUES.md)): today a gateway is reached only through the `RUPU_ANTHROPIC_BASE_URL_OVERRIDE` environment variable, which redirects **every** Anthropic account in the process, not just the one you meant. So while that override is set, put `prompt_cache = false` on every Anthropic account the process actually uses. A separate gateway account (for example `[providers.anthropic-oracle]`) does not isolate the gateway traffic, because the override applies to all of them.
- **`default_model`** (`Option<String>`): model used when an agent file omits `model:`. No global default — the agent must either set `model:` or have one resolvable here.
- **`[[providers.<name>.models]]`** (`Vec<CustomModel>`): register private/internal/fine-tuned models that aren't returned by `/v1/models`. Each entry takes `id` (required) plus optional `context_window` (input-token limit) and `max_output`. A value greater than zero also overrides the limit discovered from the provider's model list for that model, field by field (see [Model limits](#model-limits)); an agent's `contextWindowTokens` / `maxTokens` still wins over both.

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
and `max_output` are optional. When omitted, rupu reads `max_model_len` from
the server's `/v1/models`; if that's missing too, the limits are unknown and
the run says so. These surface in `rupu models list --provider oracle`.

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

`rupu models list` (and the control plane's Models tab) build their catalog of known models from three sources, in order:
1. **Custom** — `[[providers.<name>.models]]` entries from `~/.rupu/config.toml`.
2. **Live cache** — `~/.rupu/cache/models/<provider>.json` (TTL 1h; schema v2 records each model's id, input limit and output cap; a file with any other schema is ignored and refetched). `rupu models refresh` writes it, and a run refreshes it when it is stale or missing (unless a refetch failed in the last 5 minutes, see [When a refresh fails](#when-a-refresh-fails)). `rupu models list` only reads the cache — it never fetches. A listing that returns no models counts as a failed refresh and never overwrites the cache.
3. **Baked-in** — Copilot and Gemini ship a small built-in id list (limits unknown). Baked-in entries are merged beneath the live and custom rows and only fill ids the live list doesn't contain, so `rupu models list` can still show a few `baked-in` rows after a successful fetch; they never supply limits.

That three-source order is the catalog view only. The limits a **run** uses for its `model:` follow a different precedence — the agent's frontmatter, then the config entry, then the live list, then unknown — and baked-in rows never supply a limit; see [Model limits](#model-limits).

`rupu models list` and the control plane's Models tab read only the **global** `~/.rupu/config.toml`, while a run also layers the project's `.rupu/config.toml` on top, so a project-level `[[providers.<name>.models]]` entry affects runs but does not appear in the catalog view.

```sh
rupu models list              # built-in vendors + every declared account
rupu models list --provider openai
rupu models refresh           # re-fetch live caches
rupu models refresh --provider anthropic
```

`rupu models list` shows each model's input limit (`CONTEXT`), `OUTPUT` cap, `SOURCE` and — for live rows — how long ago they were `FETCHED`; an unknown limit or a non-live row's age shows `-`. The `--format json|csv` reports carry the same fields (`context`, `output`, `fetched_at`; the JSON report is version 2, with `null` for unknown values and the CSV cells left empty).

`--provider` accepts a **declared account name** as well as a built-in vendor
name — `rupu models refresh --provider anthropic-work` refreshes that account
against its declared vendor (`[providers.anthropic-work] kind = "anthropic"`),
using that account's own credential, and caches the result under the account
name so two accounts of one vendor keep separate model lists. A name that is
neither a built-in vendor nor a declared account is an error with a non-zero
exit, not a silent no-op.

An `openai-compatible` account is refreshed like any other: `rupu models
refresh` fetches its `/v1/models` with the account's Bearer key. A server with
no usable `/v1/models` reports the error for that account; its models are then
whatever `[[providers.<name>.models]]` declares.

`rupu models refresh` fetches the targeted providers in parallel and prints one line for each: `rupu: refreshed <provider> (N models)`, or `rupu: skip <provider>: <reason>` — for example a missing credential, a provider with no listing endpoint (a Gemini CLI login, a `kind = "local"` account), or an empty listing. Each provider gets 10 seconds for the whole job, credential lookup and any OAuth token refresh included. A job that outlives that is reported as `timed out after 10s` but is not cancelled, because it may be part-way through rotating a token that still has to be saved. The command keeps waiting for such jobs under one shared 30-second deadline (`rupu: waiting for N provider job(s) to finish…`), prints each as it lands (`rupu: refreshed <provider> (N models) after the wait`), and names any still running when the deadline passes. A job that finishes during that wait counts as refreshed.

The command **exits non-zero when no targeted provider was refreshed**, and zero when at least one was; the providers that failed are still printed. A script can therefore rely on the exit code to tell "refreshed" from "nothing worked".

A `model:` id that appears in none of these sources is not rejected by rupu: the id is sent to the provider as written (a provider that doesn't serve it returns its own error), and the run's `model_limits` notice flags that the model isn't in the provider's list. Run `rupu models list --provider <name>` to see the ids rupu knows, or add a `[[providers.<name>.models]]` entry.

### Model limits

Every run also needs the model's real **input limit** (used for proactive compaction) and **output cap** (the per-request `max_tokens`). Per field, rupu takes the first of:

1. the agent's `contextWindowTokens` / `maxTokens` frontmatter;
2. the `[[providers.<name>.models]]` entry (value greater than zero);
3. the provider's live model list, from the cache above;
4. unknown.

A value of `0` counts as unset at every level. An agent's `contextWindowTokens: 0` or `maxTokens: 0` is ignored (it would compact every turn, or send `max_tokens: 0`) and the notice says `ignored contextWindowTokens: 0`; a `0` in a `[[providers.<name>.models]]` entry is skipped the same way.

Each run writes a `model_limits` notice to its transcript with the values and where each came from, and says so explicitly when a limit is unknown. If the input limit is unknown, compaction is off; if the output cap is unknown, Anthropic requests carry `8192` and every other provider gets no cap (the model's max). The one exception is an OpenAI-compatible account with `stream = false`, which carries `8192` because it cannot stream. `--no-stream` changes none of this: it only changes how output is displayed, and requests still stream on the wire. Details: [agent-format.md](agent-format.md#contextwindowtokens-and-compactatpercent).

When a limit is unknown or an input was set aside, the notice also says why: the provider has no listing endpoint; the refresh failed, timed out, returned no models, or is paused (see [When a refresh fails](#when-a-refresh-fails)); the model isn't in the provider's list (`model '<id>' is not in <provider>'s model list`, shown unless your config actually supplies a limit for it); the provider lists the model without any limits (`<provider> lists '<id>' without limits; set contextWindowTokens/maxTokens or [[providers.<name>.models]]`); or the cache couldn't be written.

Where each provider's limits come from:

| Provider (auth) | Endpoint | Input limit | Output cap |
| --- | --- | --- | --- |
| anthropic (API key; SSO uses the same endpoint, but SSO verification is pending) | `GET /v1/models` | `max_input_tokens` | `max_tokens` |
| openai (ChatGPT SSO) | `/backend-api/codex/models` | `context_window` × the model's effective-context percentage (95% when absent) | not reported |
| openai (API key) | the same Codex catalog first, then ids-only `GET /v1/models` | as above; unknown on the ids-only fallback | not reported |
| copilot | live `GET {api}/models` | `max_prompt_tokens` | `max_output_tokens` |
| gemini (AI Studio API key) | `GET /v1beta/models` | `inputTokenLimit` | `outputTokenLimit` |
| gemini (Gemini CLI / Antigravity SSO) | none — Code Assist has no listing | unknown | unknown |
| openai-compatible (vLLM, …) | `GET {base_url}/v1/models` | `max_model_len` (fills only the values config left unset) | not reported |

- **OpenAI with an API key** also sends the key, as a Bearer token, to `chatgpt.com/backend-api/codex/models` — the only endpoint beyond `api.openai.com` — to read the Codex catalog's limits. That first call has its own 3-second timeout, so the fallback still fits the 10-second listing budget: if it fails, times out or lists nothing usable, rupu falls back to the ids-only `GET /v1/models`.
- **Gemini CLI / Code Assist login** exposes no model limits. Declare them yourself with `[[providers.gemini.models]]` (or pin `contextWindowTokens` / `maxTokens` on the agent); with that login, `rupu models refresh --provider gemini` reports that there is no listing rather than pretending to fetch one.
- **Copilot** reports a prompt limit that is often well below the model's full window (for example 128K of a 400K window); rupu uses the prompt limit as the input limit, and a limit the listing reports as `0` counts as absent, so it falls through to the window size. Its built-in ids only fill gaps beneath the live list and carry no limits.
- **OpenAI-compatible** limits are unknown unless config or the server provides them — there are no made-up defaults. A LoRA entry with no `max_model_len` of its own takes its parent's.
- **A listing rupu can't make sense of is an error, never an empty catalog**: a body that isn't JSON, or — for Anthropic, Codex, Copilot and OpenAI-compatible servers — one with none of the arrays the provider documents (for example a vLLM server whose `/v1/models` has no `data`). `rupu models refresh` reports it, and the existing cache is kept. A listing that parses but holds no models at all is a failed refresh too, never a cached empty list.
- **Paged listings** (Anthropic, Gemini AI Studio) are fetched 1000 models per page; a server that repeats a page cursor, or lists a model id twice, is cut off rather than looped on or double-listed.

**In the control plane:** Settings → Models lists every provider's cached models with input limit, output cap, source and fetch age, with a **Refetch** button per provider and a **Refetch all** button. It is backed by `GET /api/models` and `POST /api/models/refresh` (body `{}` for all providers, or `{"provider": "<name>"}` for one) and refreshes the control-plane machine's cache only — remote hosts keep their own caches and refresh them when a run launches there. A refetch always tries, even inside the 5-minute pause below; a job that outlives its 10-second wait is reported as `timed out after 10s` and keeps running in the background.

### When a refresh fails

A run that finds a model's cache stale or missing refetches it through its own client, bounded by the same 10 seconds. A refetch that fails, times out, or returns no models is remembered for 5 minutes in `~/.rupu/cache/models/<provider>.failed`. Until then runs skip the refetch — otherwise each launch would pay the timeout again while a provider is down — and use the stale entry if there is one, else unknown. The `model_limits` notice says so: `model list refresh failed 3m ago (<error>); retrying after 5m`.

A successful refetch removes the marker. `rupu models refresh` and the control plane's **Refetch** ignore it: they always try, and clear it on success. A provider that simply has no listing endpoint (a Gemini CLI login) is not a failure and is not remembered. A session resolves its limits on its first turn and keeps them for later turns, a lowered limit included — even when the turn that learned it failed. Only a stored value with no known limit at all (a first turn that hit a transient failure) is resolved again on the next turn, under the same 5-minute pause.

### When the provider rejects a request

Each run learns from the provider's own rejection. rupu recognizes these formats in the error text (providers don't document them, so each is pinned by a test):

| Provider | Error text | What it carries |
| --- | --- | --- |
| Anthropic | `prompt is too long: N tokens > M maximum` | the input limit M |
| OpenAI, Copilot | `maximum context length is M tokens … resulted in N tokens` | M |
| Copilot | `prompt token count of N exceeds the limit of M` | M |
| vLLM | `Input length (N) exceeds model's maximum context length (M)` | M |
| Gemini | `input token count (N) exceeds the maximum number of tokens allowed (M)` (a bare token count without `exceeds` is not an overflow) | M |
| any | `prompt is too long`, `too many tokens`, `context window` | no number |

When the error carries the real maximum and it is below the run's input limit (or that limit is unknown), rupu lowers the input limit to it for the rest of the run, writes a `model_limits_clamped` notice, compacts with a summary, and retries. With no usable number but a known input limit it still compacts once per turn; deleting the oldest exchanges is the last resort. The observed value is never written to the model cache, because it reflects the account rather than the model. A session keeps it for later turns.

Two Anthropic cases are not input overflows and are handled differently:

- **`input length and `max_tokens` exceed context limit: A + B > C`** (older Claude models; reachable because rupu now sends the model's real output cap). The input fits and the output reservation does not, so the input limit is left alone. rupu retries the turn once with `max_tokens` lowered to `C − A − 1000` and writes a `model_limits_clamped` notice saying the output cap was lowered for this request. If that leaves less than 1,024 tokens, or the retry fails the same way, it compacts like an input overflow.
- **429 `Extra usage is required for long context requests`.** The account has no extra-usage billing and the request carried the 1M-context beta (`contextWindow: 1m` with an API key, a `[1m]` model suffix with SSO). It is neither a rate limit nor a size problem, so it is never retried with the beta: rupu stops sending the beta for the rest of that client's life, clamps the input limit to 200,000, says so in a `model_limits_clamped` notice, and retries the turn once. If the input is still over 200K, the normal "prompt too long" handling compacts. Each run builds a new client and so pays one refused request; remove `[1m]` (or `contextWindow: 1m` from the agent) to skip it. A session keeps the model and window it started with, so apply that change in a new session.

The `[1m]` suffix is only a client-side marker for the 1M beta: it is stripped from the model id sent to Anthropic and from the lookup in the model list.

## Troubleshooting

**`rupu auth status` shows `✓` but `rupu run` errors with Unauthorized.**
The token may have expired faster than the refresh window expected. Re-login with `rupu auth login --account <name> --mode sso`. If api-key, the key was rotated server-side — generate a new one and re-login.

**SSO login fails on a server / over SSH.**
The browser-callback flow can't reach a desktop. Use `--mode api-key`. Copilot's device-code SSO is the only flow that works headless — visit the URL from any browser anywhere and the polling completes.

**Provider rejects the model id.**
rupu sends the id as written (it never rejects an unknown one itself), so check it against `rupu models list --provider <name>`. For a private or fine-tuned model that list doesn't include, add the model under `[[providers.<name>.models]]` in `~/.rupu/config.toml` with at least the `id` field, then retry. Custom entries always take precedence over live and baked-in.

**Gemini API-key login fails.**
Check that the key came from Google AI Studio (`AIzaSy…`) and re-login with `rupu auth login --provider gemini --mode api-key`. If the key is fine, `--mode sso` (Gemini CLI / Antigravity) is the alternative — but note that path exposes no model limits (see [Model limits](#model-limits)).

**I'm logged out after upgrading from an older rupu on macOS.**
Older versions kept credentials in the macOS keychain; rupu now uses only `~/.rupu/auth.json`, so those entries are not read. `rupu auth login` tells you when it finds leftovers. Log in again, then remove the old entries with `security delete-generic-password -s rupu -a <account>` if you like. (`rupu auth backend --use keychain` is rejected: there is nothing to select.)

**`rupu auth logout --all` removes credentials I didn't expect.**
By design — `--all` iterates every stored account × mode. Use `--account <name>` (with optional `--mode <m>`) for surgical removals.

## Deferred / future

- Richer usage visualization / dashboards beyond `rupu usage` — per-response usage is captured in JSONL transcripts and joined with workflow-run metadata today; future work is higher-level visualization, not the base reporting command.
- Local-model provider (Ollama / llama.cpp) — out of scope for Slice B-1; planned for a later slice.
- Cost accuracy enhancements — `rupu usage` reports USD from built-in/default pricing tables today; users with strict accounting needs should override provider/model pricing in config.
- Cross-provider model aliases (e.g., `model: smart`) — not planned; explicit model names are clearer.
- Vendor-specific model features (Anthropic prompt-cache toggles, OpenAI structured-output mode, Gemini grounding) — adapters expose them as opaque pass-through fields where natural; no first-class rupu surface yet.
