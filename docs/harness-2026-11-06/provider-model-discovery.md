# Provider model discovery

## Contract and diagnosis

SSO is not required to enumerate models. The selected Rupu instance owns provider credentials and discovery. Native clients read `GET /api/models` and refresh an individual authenticated account with `POST /api/models/refresh` and `{ "provider": "<account-name>" }`, then reread the authoritative catalog. A successful HTTP response can contain a failed provider outcome; clients must inspect `ok` and `error`.

The read route is cache-only. A provider which has never been refreshed has `fetched_at: null` and `stale: false`; callers must account for missing and empty live catalogs, not just the stale flag. Configured named providers are already supported by the runtime resolver and catalog. API-key discovery uses the normal selected account and its configured endpoint; this change does not override account selection.

The real API-key Anthropic account returned 16 models from a targeted refresh on 2026-11-06. The real OpenAI account has both API-key and valid SSO metadata; the existing resolver selects SSO. Both running control planes returned an empty legacy OpenAI catalog before this change. These results distinguish cache refresh and provider discovery from credential presence.

## OpenAI compatibility change

The actual failure was the hardcoded Codex `client_version=0.50.0`. The server filters model availability by minimum client version. A read-only comparison using the same existing Rupu `openai/sso` credential and account header on 2026-11-06 established:

| Catalog request | HTTP status | Returned models | Picker-visible models |
| --- | --- | --- | --- |
| Codex backend, version 0.50.0 | 200 | 0 | 0 |
| Codex backend, version 0.166.0 | 200 | 15 | 13 |
| Codex backend, version 0.166.0 plus Codex originator | 200 | 15 | 13 |

The installed Codex CLI reports 0.166.0, and its tagged official source passes its client version to the same backend model-list route. Rupu now uses that verified catalog protocol baseline through one shared URL builder, used by both listing APIs and the API-key metadata attempt. The CP does not need a Codex installation. This is a compatibility pin that must be maintained when the upstream catalog changes; it is not Rupu's package version.

The previous attempt to fall back to public `/v1/models` has been removed. Parent integration demonstrated that the existing Codex OAuth credential lacks the `api.model.read` scope required there. The newer public ChatGPT plan-usage documentation applies to a different credential flow and did not establish compatibility with this stored login.

Discovery preserves the selected token, account, and configured endpoint. OAuth never switches to an API key or another account, and custom endpoint overrides remain isolated. Rich catalog entries with explicit non-list visibility are omitted; older/custom/API-key entries without visibility remain supported. Server ordering, available context metadata, API-key `data` listings, and `supported_in_api` filtering remain intact. Unknown capability metadata is not fabricated.

Primary reference: [Codex 0.166.0 model-list implementation](https://github.com/openai/codex/blob/rust-v0.166.0/codex-rs/codex-api/src/endpoint/models.rs). The parser consumes the returned `models` array; this endpoint has no model-list pagination contract. No inference routing changed, and a listed model does not establish successful inference.

## Validation

The superseded fallback passed its 30 unit tests but failed live acceptance with a scope error; those tests did not prove the credential-flow assumption. The corrected version-gate implementation has new regressions for current-versus-obsolete requests, preservation of a configured custom endpoint and selected bearer/account header, and visibility filtering across both listing methods. Existing API-key fallback and token-refresh persistence coverage is retained. `rustfmt` and scoped `git diff --check` pass; the corrected provider test filter is pending the shared target becoming available.

The direct live probe proves the version gate and real account model counts. A targeted refresh through the rebuilt CP and its authoritative cache remains pending parent integration. No completion request, agent job, CP restart, provider reauthentication, or release was performed by this workstream. Probes printed only status/count/schema metadata, not credentials or account IDs.

The final corrected implementation passed all 27 `fetch_models_tests` on the combined tree. The earlier 30-test result covered the removed public fallback and is superseded. No model completion was part of this check.
