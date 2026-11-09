# Operator issue reader — source-only G1 slice

Baseline: `a42e3beaa5736998fabcf9513976545044e326de`. This is an additive, read-only owning-instance contract. It does not complete G1: GitLab, Linear and Jira discussion adapters remain unsupported, individual comments are clipped at 8 KiB, and no native/live acceptance is claimed.

## Authority and provider support

`issues.operator_read.v1` is advertised only when the optional issue-reader port is installed through the owning repository adapter. The CLI implements that port using the existing instance's configured SCM registry and explicit account selection. No account, workspace, project or origin is inferred from a display name or a recorded run. Recorded trigger payloads remain historical context; these routes return a separate current observation.

GitHub, GitLab, Linear and Jira support issue bodies. Only GitHub supports comment pages. Configurations without a safe API endpoint are listed as unavailable. Account presence is not authorization: the provider read may return forbidden/not found/rate limited. `expected_origin` is an equality guard against the configured adapter endpoint; it is never used as a caller-selected transport URL.

The operator path uses dedicated no-redirect, instrumented clients and bounded response decoding; existing model tools retain their behavior. Jira validates the configured site, port and context path before attaching credentials, then checks the full returned issue key. GitHub checks returned API issue/repository URLs and number; GitLab first verifies project path/id, then issue project_id/iid; Linear verifies selected team id/key and the returned issue's full identifier and team. Aliases, moved resources and inconsistent identities fail closed rather than being labeled as the requested issue. GitHub comments must return the exact issue_url. Credential refresh remains owned by the existing GitLab token source.

## Routes and frozen schema

All routes accept only absent/empty/`local` host; another host is explicitly unsupported. They require a direct connection to the owning instance and do not fall back to a coordinator's local registry.

- `GET /api/issues/accounts?host=local`: `{host_id, truncated, accounts:[{tracker,account,origin,body_supported,comments_supported,unavailable_reason}]}`. No provider request. Maximum 64 valid account rows.
- `GET /api/issues/:tracker/:number?host=local&project=...&account=...&expected_origin=...`: issue metadata, first body chunk and first comment page.
- `GET /api/issues/:tracker/:number/body?...&cursor=...`: body continuation.
- `GET /api/issues/:tracker/:number/comments?...&cursor=...`: discussion continuation.

Each content response echoes `identity:{host_id,tracker,project,account,number,origin}`; `number` is a canonical decimal **string**, preserving the full u64 range. It also includes `source:"current_issue"`, `observed_at`, `freshness:"connector_cache_policy"` and `version` (SHA256 of the returned bounded Issue serialization). Linear fetches up to 65 labels so the 65th is a truncation sentinel for the 64-label display budget; issue versions cover the fetched bounded fields, not all provider metadata or comment history. The operator adapters currently bypass their optional body caches; the freshness label does not promise a historical snapshot or synchronization with recorded triggers.

Initial `issue` contains `title,state,labels,author,created_at,updated_at,metadata_truncated,body`. `body` is `{text,offset,byte_count,total_bytes,truncated,next_cursor}`; offsets and counts are UTF-8 bytes. Continuations return a top-level `body` instead of `issue`.

`comments` is `{state,reason,items,next_cursor,pagination:"live",order:"oldest_first",completeness}`. State is available/unsupported/unavailable; completeness is page_only/end_observed/unavailable. Each item is `{id,author,body,body_truncated,metadata_truncated,created_at,author_association}`; id is an opaque string. Unsupported or failed discussion never fabricates an empty successful thread. The initial response preserves body availability when discussion fails. A full final permitted page has no continuation, `page_only`, and reason `comment_page_limit`; only a short provider page yields `end_observed`.

## Budgets and continuation semantics

- Maximum two concurrent reader requests per process; excess requests return busy without waiting.
- Overall 20 second deadline, 12 seconds for issue fetch, 6 seconds for comment fetch; each operator HTTP client also has a 10 second timeout.
- Each upstream response, including error bodies, is streamed into a maximum 2 MiB buffer before JSON decoding. A route makes at most two logical issue reads and one comment read (6 MiB decoded response-body budget); GitLab's existing token source may retry a refused authentication once without consuming the refused body. No automatic pagination or redirect follows.
- Final serialized JSON response maximum 512 KiB, including escape expansion. Oversized output returns response_too_large rather than an oversized response.
- Body chunk 64 KiB; comment page 50 items, comment body 8 KiB, at most 10000 comment pages. Title 4096 bytes, author 512, labels 64×256, association128. Truncation is explicit; clipped comment bodies have no continuation in this slice.
- Cursor HMAC covers the full target/account/origin, stream kind, position, current issue version, owning global directory and live adapter epoch. A random process key invalidates cursors on restart. Each live adapter allocation has a random weak-owner epoch, so different instances sharing a global directory and recycled allocation addresses cannot share a continuation. Expiry is 15 minutes.
- Every continuation rereads the current issue, checking the version before returning content. Changed issue content yields stale_cursor. Comment pages are live offset pages, not stable snapshots: concurrent additions/removals can cause movement, and an observed short page does not establish future completeness.

Errors use sanitized `{code,error}`. Codes: invalid_target/invalid_cursor (400), stale_cursor/selection_changed (409), not_found (404), forbidden (403), rate_limited (429), unsupported (501), adapter_missing/account_unavailable/busy (503), timeout (504), upstream_unavailable/response_too_large (502). Raw upstream bodies, credentials and exception strings are not included.

## Verification state and next acceptance

Source was parsed/formatted with rustfmt and checked for whitespace errors. Independent source review was requested before any execution. Unit fixtures were authored for exact account/origin selection before calls, returned identity mismatch, signed cursor tampering/target/instance/kind/expiry, adapter lifetime epoch, UTF-8 chunk reassembly, response escaping budgets, bounded upstream/error response handling, safe origins and Jira site/normalization guards. Five additional provider-shaped HTTP fixtures cover actual returned identity mismatches in all four providers and rejection of a Jira authenticated redirect; these are authored but unexecuted.

**No compiler, test suite, live provider request, service restart or deployment was run for this slice.** The fixtures are source, not passing-test evidence. Before enabling acceptance claims, compile the integrated tree and execute the fixtures plus provider-shaped identity/redirect/oversize HTTP fixtures for each adapter, then validate native exact selection, stale continuation, unsupported discussion, authorization failure and changed-account journeys. Broader G1 requires genuine GitLab/Linear/Jira comment adapters and complete long-comment reading.
