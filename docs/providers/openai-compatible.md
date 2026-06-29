# OpenAI-compatible providers (Oracle GenAI, vLLM, …)

The `openai-compatible` kind lets rupu talk to any HTTP server that speaks
the OpenAI `/v1/chat/completions` API with a static Bearer key. Common
targets include self-hosted vLLM, Oracle GenAI, Together AI, Fireworks AI,
and OpenRouter.

## Prerequisites

- A running `/v1/chat/completions`-compatible server (or a hosted service
  that provides a base URL and an API key).
- The server must accept `Authorization: Bearer <key>` in the request header.

## Step 1: Add the provider to `~/.rupu/config.toml`

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

`base_url` may include or omit a trailing `/v1` — rupu normalises both forms
to `<root>/v1/chat/completions`.

Set `stream = false` for servers (or server versions) that do not implement
the server-sent-event (SSE) streaming endpoint. rupu will send a standard
blocking request and synthesise the same event sequence for the agent loop.

Each `[[providers.oracle.models]]` entry requires `id`; `context_window`
and `max_output` are optional (rupu applies defaults of 32768 and 8192
respectively when omitted). These appear in `rupu models list
--provider oracle` with source `custom`.

You can name the provider anything — replace `oracle` with `vllm`, `together`,
`fireworks`, etc. The name becomes the `provider:` value in agent files and
the suffix of the credential env var.

## Step 2: Store the API key

```sh
# Interactive prompt (key is not echoed):
rupu auth login --provider oracle --mode api-key

# Or pipe from stdin:
echo -n "$MY_API_KEY" | rupu auth login --provider oracle --mode api-key
```

The key is written to `~/.rupu/auth.json` (chmod 600). To verify:

```sh
rupu auth status
```

For CI or ephemeral environments, skip `rupu auth login` and set the env var
directly — rupu reads it automatically:

```sh
export RUPU_ORACLE_API_KEY=sk-...
```

The pattern is `RUPU_<UPPERCASED_PROVIDER_NAME>_API_KEY`.

## Step 3: Create an agent file

```markdown
---
name: oracle-codereview
description: Code review via Oracle GenAI.
provider: oracle
model: /raid/models/zai-org/GLM-5.2-FP8
---

You review code changes for correctness, style, and missing tests.
```

## Step 4: Run

```sh
rupu run --agent oracle-codereview
```

To verify the model list:

```sh
rupu models list --provider oracle
```

## Configuration reference

| Field           | Type     | Required | Description                                                                 |
| --------------- | -------- | :------: | --------------------------------------------------------------------------- |
| `kind`          | string   | yes      | Must be `"openai-compatible"`.                                              |
| `base_url`      | string   | yes      | Root of the API server (`http://host:port` or `…/v1`).                     |
| `default_model` | string   | yes      | Model id sent when the agent file omits `model:`.                           |
| `stream`        | bool     | no       | Enable SSE streaming (default `true`). Set `false` for servers without SSE. |

Each `[[providers.<name>.models]]` entry:

| Field            | Type   | Required | Description                    |
| ---------------- | ------ | :------: | ------------------------------ |
| `id`             | string | yes      | Model id passed verbatim to the API.  |
| `context_window` | u32    | no       | Maximum context in tokens (default 32768). |
| `max_output`     | u32    | no       | Maximum output tokens (default 8192). |

## Limitations

- Only API-key auth (`Authorization: Bearer`) is supported. SSO flows are
  not available for openai-compatible providers.
- Workflow steps and subagents will gain openai-compatible support in Plan 2.
  For now they use the built-in providers (anthropic, openai, gemini, copilot)
  only.
- Model listing (`rupu models list`) returns only the models declared in
  `[[providers.<name>.models]]` — rupu does not call `/v1/models` on these
  endpoints.
- Cost tracking reports $0.00 for openai-compatible providers (no pricing
  tables are available). Usage token counts are still captured in JSONL
  transcripts if the server returns them.

## Troubleshooting

**`ProviderError::Api { status: 401, … }`**
The Bearer key is missing or wrong. Check the key stored in `auth.json`
(`rupu auth status`) or the `RUPU_ORACLE_API_KEY` env var.

**`ProviderError::Http(…)` or connection refused**
The `base_url` is unreachable. Verify the server is running and the URL is
correct.

**Model not found**
openai-compatible providers don't query `/v1/models`. Add the model id under
`[[providers.oracle.models]]` in `~/.rupu/config.toml`.

**No text in response (`resp.text()` is `None`)**
Some servers return tool-call blocks even for plain text prompts. Check the
raw response with `--verbose` (coming in Plan 2) or inspect the JSONL
transcript.
