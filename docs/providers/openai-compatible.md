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
blocking request and synthesise the same event sequence for the agent loop. Such a server has no streaming to keep a long generation alive, so a request with no known output cap carries `max_tokens: 8192` (the pre-discovery default) to keep one response inside the HTTP timeout; servers that stream omit the field.

Each `[[providers.oracle.models]]` entry requires `id`; `context_window`
and `max_output` are optional. When omitted, rupu reads `max_model_len` from
the server's `/v1/models` and uses it as the input limit; if that is missing
too, the limit is unknown (there is no made-up default) and the run's
`model_limits` transcript notice says so. These appear in `rupu models list
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
rupu run oracle-codereview
```

To verify the model list:

```sh
rupu models refresh --provider oracle   # fetches the server's /v1/models
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
| `context_window` | u32    | no       | Input-token limit, used for compaction. When omitted, the server's `/v1/models` `max_model_len`; unknown if that is missing too. |
| `max_output`     | u32    | no       | Maximum output tokens. When omitted, unknown and no cap is sent (the server caps output to fit). |

## Limitations

- Only API-key auth (`Authorization: Bearer`) is supported. SSO flows are
  not available for openai-compatible providers.
- Workflow steps and dispatched subagents support openai-compatible providers
  the same way `rupu run` does — a step whose agent sets `provider: oracle`
  resolves `[providers.oracle]` from config and builds the same client.
- `rupu models refresh` fetches the server's `/v1/models` (with the account's
  Bearer key) and caches it for an hour; `rupu models list` reads that cache
  plus the models declared in `[[providers.<name>.models]]`. A server without
  a usable `/v1/models` makes refresh report an error for that account, and
  its models are then only those declared in config. Only `max_model_len` is
  read from the listing (as the input limit); no output cap is reported.
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
The model id is sent to the server verbatim, so it must be one the server
serves. Run `rupu models refresh --provider oracle` then
`rupu models list --provider oracle` to see the ids the server reports (a run
whose model isn't in that list says so in its `model_limits` notice). If the
server has no `/v1/models`, nothing is listed — declare the model under
`[[providers.oracle.models]]` in `~/.rupu/config.toml` so its limits are known.

**No text in response (`resp.text()` is `None`)**
Some servers return tool-call blocks even for plain text prompts. Check the
raw response with `--verbose` (coming in Plan 2) or inspect the JSONL
transcript.
