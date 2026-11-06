# Provider model discovery

## Contract and diagnosis

SSO is not required to enumerate models. The selected Rupu instance owns provider credentials and discovery. Native clients read `GET /api/models` and refresh an individual authenticated account with `POST /api/models/refresh` and `{ "provider": "<account-name>" }`, then reread the authoritative catalog. A successful HTTP response can contain a failed provider outcome; clients must inspect `ok` and `error`.

The read route is cache-only. A provider which has never been refreshed has `fetched_at: null` and `stale: false`; callers must account for missing and empty live catalogs, not just the stale flag. Configured named providers are already supported by the runtime resolver and catalog. API-key discovery uses the normal selected account and its configured endpoint; this change does not override account selection.

The real API-key Anthropic account returned 16 models from a targeted refresh on 2026-11-06. The real OpenAI account has both API-key and valid SSO metadata; the existing resolver selects SSO. Both running control planes returned an empty legacy OpenAI catalog before this change. These results distinguish cache refresh and provider discovery from credential presence.

## OpenAI compatibility change

Only default production ChatGPT OAuth clients gain a fallback to `https://api.openai.com/v1/models` when their legacy catalog is empty, malformed, fails, or times out. The fallback uses the same selected access token. It does not switch to an API key, another account, or another credential store. Custom endpoint overrides do not enable this fallback, and no legacy account-ID header is sent to the public route.

The current public ChatGPT schema uses a `models` array. The parser keeps entries explicitly marked `visibility: "list"`, uses canonical `slug` values, and preserves server ordering. It does not invent model capabilities or limits missing from the response. Existing API-key discovery, including `data` listings and `supported_in_api` filtering, remains intact. The documented public listing has no pagination contract.

Primary reference: [OpenAI — Models and inference](https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference). This change is model discovery only; it does not change inference routing or claim successful inference with any listed model.

## Validation

All 30 tests in `openai_codex::fetch_models_tests` pass, including five new regressions for public visibility/order, exact production routing, same-token fallback without a legacy account header, preservation of usable legacy catalogs, and error propagation. Existing API-key fallback and token-refresh persistence tests remain green. Tests use the shared historical target and the current combined tree. The first sandbox run could not bind local HTTP mock listeners; the rerun with local networking allowed passed. `rustfmt` and the scoped `git diff --check` also pass.

A targeted model-only refresh through the updated runtime remains pending parent integration. No completion request, agent job, CP restart, provider reauthentication, or release was performed by this workstream.
