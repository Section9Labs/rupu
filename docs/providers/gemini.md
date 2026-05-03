# Google Gemini

## Status: SSO only in Plan 1

The `GoogleGeminiClient` lifted from phi-providers targets the Vertex AI / Gemini CLI OAuth path. **API-key authentication via AI Studio is not yet wired** — using `--mode api-key` returns `NotWiredInV0` with a pointer to this file. See `TODO.md` for the deferred AI-Studio constructor work.

## SSO via Google account

```sh
rupu auth login --provider gemini --mode sso
```

A browser opens to `accounts.google.com/o/oauth2/v2/auth`. Authorize the rupu OAuth app for the `cloud-platform` scope. The redirect populates rupu's localhost listener; the OAuth token is stored at `rupu/gemini/sso`.

The token works against the Vertex AI endpoint. You'll need a Google Cloud project with the Vertex AI API enabled and billing configured — the OAuth scope grants access but doesn't substitute for project setup.

## Configuration

Set the `project_id` if your token doesn't carry one in its `extra` claims:

```toml
[providers.gemini]
region = "us-central1"
default_model = "gemini-2.5-pro"
```

The `project_id` is read from the OAuth token's `extra` field (populated during the SSO flow). For headless setups where you can't run the SSO flow, this is the deferred AI-Studio API-key path that's not yet supported.

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

`rupu models list --provider gemini` shows the curated baked-in v0 list:
- `gemini-2.5-pro`
- `gemini-2.5-flash`
- `gemini-1.5-pro`

Once the AI-Studio listing endpoint is wired, `rupu models refresh --provider gemini` will pull the live list.

## Known quirks

- **Vertex AI region** — set via `region` in config. Default is the value baked into the lifted client; setting `us-central1` explicitly avoids surprises.
- **AI Studio API-key endpoint** — different shape from Vertex; Plan 1 doesn't ship a separate `GoogleGeminiAiStudioClient`. Track via `TODO.md`.
- **Project ID required** — Google's API rejects requests without a billing-enabled project; the SSO flow captures this in token claims, but ensure your Google Cloud project has Vertex AI enabled before first run.
