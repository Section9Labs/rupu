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

- **Gemini CLI** (what `rupu auth login` gives you — it stores `"variant": "gemini-cli"` — and what any stored credential without a `variant` field uses): requests go to `https://cloudcode-pa.googleapis.com` (`/v1internal:generateContent` and `/v1internal:streamGenerateContent`), and the token is refreshed with the Gemini CLI client's id and secret.
- **Antigravity**: requests go to the sandbox endpoint `https://daily-cloudcode-pa.sandbox.googleapis.com`, with Antigravity's own client id and secret. rupu selects it only when the stored credential carries `"variant": "antigravity"`, which `rupu auth login` never writes (it signs in as the Gemini CLI client); such a credential is set up with the same calls as below, against the sandbox endpoint — that path is not verified against the live service.

### Code Assist project

Every Code Assist request names a `project`: the **Google Cloud project ID** Google bills and meters the account's Code Assist usage against. It is not a rupu project, a directory, or a repo, and the API-key (AI Studio) path has none.

rupu finds it the way the Gemini CLI does (its `setupUser`, `packages/core/src/code_assist/setup.ts`):

1. `POST /v1internal:loadCodeAssist` asks which tier and project the account has. An account that is already set up gets its project straight back.
2. An account with no tier yet is onboarded with `POST /v1internal:onboardUser`, using the tier Google marks as default. The free tier is onboarded without a project — Google provisions a managed one — and the call returns a long-running operation that rupu polls (`GET /v1internal/operations/…`, every 5s, giving up after 5 minutes) until it reports the project.

**When it runs.** `rupu auth login --provider gemini --mode sso` runs it right after the browser sign-in and stores the result in the credential (`project_id` in `~/.rupu/auth.json`, next to `variant`). A credential stored without one — a login from before this, or one whose setup failed — is set up by the first Code Assist request instead, and the project is recorded in the credential then (under the `auth.json.lock` lock, and only if the stored credential is still the same login), so later runs skip the setup. Token refreshes keep it. A stored project is reused as is (the Gemini CLI repeats the setup every session; rupu does not): if the account's project changes — say, it moves off the free tier onto your own project — set `GOOGLE_CLOUD_PROJECT`, or run `rupu auth login --provider gemini --mode sso` again to store the new one.

**`GOOGLE_CLOUD_PROJECT`.** Set `GOOGLE_CLOUD_PROJECT` (or `GOOGLE_CLOUD_PROJECT_ID`) to use your own project — a paid (standard-tier) account needs one. While it is set, the setup asks for that project and requests use it; it overrides the stored project and is never written to `auth.json`. It must be the project ID (`my-project-123`), not the numeric project number. `config.toml` has no key for the project.

**When there is none.** If Google assigns no project and none is set, the request fails before anything is sent, with an error naming `GOOGLE_CLOUD_PROJECT` and the API-key alternative (and Google's ineligibility reasons, or its account-validation link, when it gives them). rupu never sends an empty project. At login the same failure is a warning — the sign-in is kept, and the first request retries the setup:

```text
rupu: signed in, but Gemini Code Assist setup failed: … Set GOOGLE_CLOUD_PROJECT (or GOOGLE_CLOUD_PROJECT_ID) …
rupu: the first Gemini request retries the setup.
```

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
- **Code Assist project** — SSO requests carry a Google Cloud project ID that rupu sets up at login (or on the first request) the way the Gemini CLI does, overridable with `GOOGLE_CLOUD_PROJECT`; see [Code Assist project](#code-assist-project). There is no Vertex AI setup step.
