# GitHub Copilot

## Subscription requirement

Using the Copilot API requires a paid GitHub Copilot Pro / Business / Enterprise subscription. Free GitHub accounts can't invoke the underlying API even with a valid OAuth token.

## API key (GitHub PAT)

The simplest path uses your existing GitHub Personal Access Token:

```sh
rupu auth login --provider copilot --mode api-key --key ghp_XXXXX
```

Or if `gh auth login` has already configured a token in your environment:

```sh
export GITHUB_TOKEN=$(gh auth token)
rupu auth login --provider copilot --mode api-key
# (reads GITHUB_TOKEN from env when --key is omitted)
```

Stored in `~/.rupu/auth.json` (mode 0600).

## SSO via GitHub device code

```sh
rupu auth login --provider copilot --mode sso
```

This is the same flow `gh auth login` uses:

1. rupu requests a device code from `github.com/login/device/code`.
2. rupu prints:
   ```
   Visit https://github.com/login/device and enter code: ABCD-1234
   ```
3. Open the URL in a browser (any browser, anywhere — this works headless / over SSH unlike the other providers' SSO).
4. Paste the code, authorize the rupu Copilot OAuth app.
5. rupu polls until you authorize, then exchanges the GitHub token for a Copilot API token. Both are stored in `~/.rupu/auth.json` (mode 0600).

## Example agent file

```markdown
---
name: copilot-codereview
description: Code review via Copilot's models.
provider: copilot
auth: api-key
model: claude-sonnet-4
---

You review code changes for bugs, smells, and missing tests.
```

## Available models

rupu fetches Copilot's live model list (`GET {api}/models`) and caches it for an hour; `rupu models refresh --provider copilot` forces a refetch, and a run refreshes a stale cache on its own. `rupu models list --provider copilot` shows the cached list with source `live`, including each model's input limit (`max_prompt_tokens`, often well below the full window) and output cap (`max_output_tokens`) — rupu uses the input limit for compaction and the output cap as the per-request cap.

`rupu models list` also merges in a small built-in list (source `baked-in`, ids only). Baked-in entries sit beneath the live and custom rows and only fill ids the live list doesn't contain — so they are all you see until the first fetch succeeds — and they never supply limits:

- `gpt-4o`
- `gpt-4o-mini`
- `claude-sonnet-4`
- `o4-mini`

If your Copilot subscription grants access to other models that aren't listed (e.g., enterprise-only), register them as custom entries. A `context_window` / `max_output` set here overrides the discovered value for that model:

```toml
[[providers.copilot.models]]
id = "claude-opus-4.5"
context_window = 200000
```

## Configuration knobs

```toml
[providers.copilot]
default_model = "claude-sonnet-4"
timeout_ms = 120000
max_retries = 5
max_concurrency = 4
```

For Copilot Enterprise tenants:

```toml
[providers.copilot]
# enterprise_url is read from the OAuth token's extra field; set here
# to override or for api-key flows where it can't be inferred.
```

(The `enterprise_url` override flows through `AuthCredentials::OAuth { extra }`. For api-key flows this isn't yet exposed via TOML — file an issue if you need it.)

## Known quirks

- **Copilot token expiry.** The exchanged Copilot API token expires faster than the GitHub PAT used to mint it. rupu caches the Copilot token internally and re-mints from the GitHub token on expiry. If you see frequent re-prompts, your GitHub token may have rolled — `gh auth refresh` and re-login.
- **Rate limits.** Copilot's rate limits aren't publicly documented. The default `max_concurrency = 4` is a safe ceiling; users on higher subscription tiers may bump this.
