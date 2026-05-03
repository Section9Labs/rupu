# Provider reference

Slice B-1 adds four LLM providers, each supporting two authentication modes. This document is the canonical reference for what works, how to configure it, and how to debug it. For step-by-step walkthroughs, see `docs/providers/<name>.md`.

## Provider × auth-mode matrix

| Provider  | API key | SSO  | SSO flow         | Notes                                                              |
| --------- | :-----: | :--: | ---------------- | ------------------------------------------------------------------ |
| anthropic |   ✓     |  ✓   | Browser callback | Console API key OR Claude.ai SSO.                                  |
| openai    |   ✓     |  ✓   | Browser callback | Platform API key OR ChatGPT SSO. Different endpoints under hood.   |
| gemini    |   —     |  ✓   | Browser callback | API-key path via AI Studio is deferred (see `TODO.md`). SSO via Vertex/CLI works. |
| copilot   |   ✓     |  ✓   | Device code      | API-key path uses a GitHub PAT (`GITHUB_TOKEN`). Requires paid Copilot. |

Anthropic remains the most exercised provider; Copilot's API-key path is most reliable for users who already have `gh auth login` configured. Gemini API-key support is queued for a follow-up release.

## Auth flows

### API key

```sh
rupu auth login --provider <name> --mode api-key --key <secret>
# or omit --key to read from stdin
echo -n "$KEY" | rupu auth login --provider <name> --mode api-key
```

Stored in the OS keychain at `rupu/<provider>/api-key`.

### SSO browser callback (Anthropic, OpenAI, Gemini)

```sh
rupu auth login --provider <name> --mode sso
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
rupu auth login --provider copilot --mode sso
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
3. Error: `no credentials configured for <provider>. Run: rupu auth login --provider <name> --mode <api-key|sso>`.

To force a specific mode, set `auth: api-key` or `auth: sso` in the agent's YAML frontmatter.

### Refresh

SSO access tokens expire (typically 1 hour). The resolver pre-emptively refreshes when `expires_at - now < 60s` on a `get()` call, using the stored refresh token. On refresh failure: `<provider> SSO token expired and refresh failed: <reason>. Run: rupu auth login --provider <name> --mode sso`. There is no automatic fall-back to API-key — the user explicitly chose SSO.

### Logout

```sh
rupu auth logout --provider <name>             # both api-key and sso
rupu auth logout --provider <name> --mode sso  # just one
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
region = "us-central1"   # for Vertex AI users

[providers.copilot]
# typically nothing needed

[[providers.openai.models]]   # custom/private models
id = "gpt-5-internal-finetune"
context_window = 200000
max_output = 16000
```

### Field reference

- **`base_url`** (`Option<String>`): override the vendor's default API endpoint. Useful for proxies and Azure-OpenAI-style deployments. Default: vendor's documented URL.
- **`org_id`** (`Option<String>`, OpenAI): organization scope for billed usage.
- **`region`** (`Option<String>`, Gemini): Vertex AI region (e.g., `us-central1`, `europe-west4`). Ignored on AI Studio.
- **`timeout_ms`** (`Option<u64>`): per-request timeout. Default: `120000` (2 min).
- **`max_retries`** (`Option<u32>`): max attempts on `Transient` / `RateLimited` errors before giving up. Default: `5`.
- **`max_concurrency`** (`Option<usize>`): per-provider semaphore size. Defaults: anthropic 4, openai 8, gemini 4, copilot 4.
- **`default_model`** (`Option<String>`): model used when an agent file omits `model:`. No global default — the agent must either set `model:` or have one resolvable here.
- **`[[providers.<name>.models]]`** (`Vec<CustomModel>`): register private/internal/fine-tuned models that aren't returned by `/v1/models`. Each entry takes `id` (required) plus optional `context_window` and `max_output`.

## Model resolution

`rupu run <agent>` resolves the agent's `model:` field through three sources in order:
1. **Custom** — `[[providers.<name>.models]]` entries from `~/.rupu/config.toml`.
2. **Live cache** — `~/.rupu/cache/models/<provider>.json` (TTL 1h). Populated by `rupu models refresh` or lazily on first `rupu models list`.
3. **Baked-in** — Copilot and Gemini ship a curated v0 list since their public listing endpoints are limited.

```sh
rupu models list              # all four providers
rupu models list --provider openai
rupu models refresh           # re-fetch live caches
rupu models refresh --provider anthropic
```

If the agent's `model:` value isn't found in any source, rupu errors with:
> `model 'xyz' not found for provider 'openai'. Run 'rupu models list --provider openai' to see available models, or add a custom entry to ~/.rupu/config.toml.`

## Troubleshooting

**`rupu auth status` shows `✓` but `rupu run` errors with Unauthorized.**
The token may have expired faster than the refresh window expected. Re-login with `rupu auth login --provider <name> --mode sso`. If api-key, the key was rotated server-side — generate a new one and re-login.

**SSO login fails on a server / over SSH.**
The browser-callback flow can't reach a desktop. Use `--mode api-key`. Copilot's device-code SSO is the only flow that works headless — visit the URL from any browser anywhere and the polling completes.

**Custom model rejected.**
Add the model under `[[providers.<name>.models]]` in `~/.rupu/config.toml` with at least the `id` field, then retry. Custom entries always take precedence over live and baked-in.

**Gemini API-key login fails.**
Plan 1 didn't wire the AI Studio API-key endpoint (the lifted client only supports Vertex/CLI OAuth). Use `--mode sso` for now, or track the deferred work in `TODO.md`.

**Cargo build prompts for keychain access on every `cargo run`.**
macOS treats each freshly-built binary as a different code identity. Track the deferred signing/notarization work in `TODO.md`. Quick fix: click "Always Allow" once on the first prompt — the trust persists per binary path until the next rebuild.

**`rupu auth logout --all` removes credentials I didn't expect.**
By design — `--all` iterates every provider × mode. Use `--provider <name>` (with optional `--mode <m>`) for surgical removals.

## Deferred / future

- `rupu usage` aggregation subcommand — captured per-response in JSONL transcripts; aggregate UI ships with Slice D.
- Local-model provider (Ollama / llama.cpp) — out of scope for Slice B-1; planned for a later slice.
- Cost calculation in dollars — token counts only today; price tables ship with Slice D.
- Cross-provider model aliases (e.g., `model: smart`) — not planned; explicit model names are clearer.
- Vendor-specific model features (Anthropic prompt-cache toggles, OpenAI structured-output mode, Gemini grounding) — adapters expose them as opaque pass-through fields where natural; no first-class rupu surface yet.
