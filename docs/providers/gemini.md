# Google Gemini

## Status: API key (AI Studio) and SSO

rupu supports two Gemini credential paths:

- **API key via Google AI Studio** (`AIzaSy…`) — talks to `generativelanguage.googleapis.com`. Store it with `rupu auth login --provider gemini --mode api-key`.
- **SSO via Google account** — the Gemini CLI / Antigravity path (Cloud Code Assist), described below.

The two differ in what they can tell rupu about the model: AI Studio publishes per-model token limits, Code Assist does not. See [Model limits](#model-limits).

## SSO via Google account

```sh
rupu auth login --provider gemini --mode sso
```

A browser opens to `accounts.google.com/o/oauth2/v2/auth`. rupu signs in as the Gemini CLI's installed-app OAuth client and asks for the `cloud-platform`, `openid` and `email` scopes. The redirect populates rupu's localhost listener; the OAuth token is stored in `~/.rupu/auth.json` (mode 0600).

The token is used against Google's **Cloud Code Assist** API, the backend the Gemini CLI itself talks to — not Vertex AI, and not `generativelanguage.googleapis.com` (that one is the API-key path). No Vertex AI project setup is involved, and rupu never calls a Vertex endpoint.

- **Gemini CLI** (what `rupu auth login` gives you, and what any stored credential without a `variant` field uses): requests go to `https://cloudcode-pa.googleapis.com` (`/v1internal:generateContent` and `/v1internal:streamGenerateContent`), and the token is refreshed with the Gemini CLI client's id and secret.
- **Antigravity**: requests go to the sandbox endpoint `https://daily-cloudcode-pa.sandbox.googleapis.com`, with Antigravity's own client id and secret. rupu selects it only when the stored credential carries `"variant": "antigravity"`; `rupu auth login` does not write that field.

Every Code Assist request carries a `project` field. rupu takes it from the stored credential's `project_id` and sends an empty string when the credential has none. `rupu auth login` does not discover or store a project id, and `config.toml` has no key for one.

## Configuration

```toml
[providers.gemini]
default_model = "gemini-2.5-pro"
```

`region` is accepted by the config parser but has no effect: it is reserved
for a Vertex AI regional endpoint, and none of rupu's Gemini paths (AI Studio,
Gemini CLI, Antigravity) is region-scoped. See `docs/providers.md`.

For headless setups where you can't run the SSO flow, use the AI Studio API-key path.

## Example agent file

```markdown
---
name: long-context-search
description: Search a 1M-token codebase with Gemini's long-context window.
provider: gemini
auth: sso
model: gemini-2.5-pro
---

You search large codebases. Cite file paths and line numbers.
```

## Available models

With an AI Studio API key, `rupu models refresh --provider gemini` (or any run, when the cache is stale) fetches the live list from `GET /v1beta/models`, following `nextPageToken`, and caches it for an hour in `~/.rupu/cache/models/gemini.json`; `rupu models list --provider gemini` reads that cache. `rupu models list` also merges in a curated built-in id list (source `baked-in`). Baked-in entries sit beneath the live and custom rows and only fill ids the live list doesn't contain — so they are all you see until the first fetch succeeds, and always on the Gemini CLI / Antigravity SSO path, which has no listing endpoint — and they never supply limits:
- `gemini-2.5-pro`
- `gemini-2.5-flash`
- `gemini-1.5-pro`

## Model limits

Every run needs the model's input limit (for compaction) and output cap. Where they come from depends on the credential:

- **AI Studio (API key):** the listing reports `inputTokenLimit` and `outputTokenLimit` per model, so rupu discovers both. They are cached for an hour, and each run's `model_limits` transcript notice states them and says they came from the live list.
- **Gemini CLI / Antigravity (OAuth, Code Assist):** the Code Assist API has **no listing method**, so rupu cannot discover limits. They are unknown unless you declare them — either per model in `config.toml`:

  ```toml
  [[providers.gemini.models]]
  id = "gemini-2.5-pro"
  context_window = 1048576   # input-token limit
  max_output = 65536
  ```

  or on the agent with `contextWindowTokens` / `maxTokens` (a `0` is ignored). With neither, the run's `model_limits` notice says the limits are unknown and compaction is off, and `rupu models refresh --provider gemini` reports that there is no listing (and exits non-zero when Gemini is the only provider you targeted, because nothing was refreshed). Gemini requests carry no output cap when none is known, so the model's own maximum applies.

Gemini budgets input and output independently, so compaction triggers at `compactAtPercent` of the input limit alone. See [providers.md](../providers.md#model-limits) for the full precedence (agent → config → live list → unknown).

## Known quirks

- **Vertex AI region** — `region` in config is parsed but unused; no shipped Gemini client targets a regional Vertex endpoint. Setting it changes nothing.
- **AI Studio vs Code Assist** — two different APIs behind one `gemini` provider. AI Studio (API key) has a model listing and reports limits; Code Assist (SSO) has neither, so declare limits in `[[providers.gemini.models]]` (see [Model limits](#model-limits)).
- **Code Assist project** — SSO requests carry a `project` taken from the stored credential's `project_id`, which `rupu auth login` does not populate (see [SSO via Google account](#sso-via-google-account)). There is no Vertex AI setup step.
