# Current issue discovery, v1

This additive operator HTTP surface lists current provider issues under an explicitly selected account and forge project. It does not infer an SCM project from a local workspace, branch, repository display name or recorded trigger. It does not create or update issues or grant model tools.

Feature: `issues.discovery.pages.v1`, advertised only when the owning `IssueReader` implements this port. Existing `GET /api/issues/accounts` rows gain `discovery_supported` and `discovery_search_supported` booleans; existing fields remain unchanged. Old backends without the feature keep discovery unavailable. The account catalog does not prove authorization; provider access checks remain authoritative.

## Request

`GET /api/issues/:tracker/discovery`

Required query fields: `project` (exact forge project,1–512 UTF8 bytes), `account` (exact configured account,1–128), `expected_origin` (exact catalog API origin,1–2048), `state` (`open`, `closed`, `all`). Optional `host` must be absent/empty/`local`; `title_query` is literal nonblank text1–128 bytes without controls; `cursor` is the returned opaque string, at most16384 bytes. Unknown query fields are refused. Tracker is `github`, `gitlab`, `jira`, or `linear`.

The selected project/account/origin and state/title query must be repeated exactly on continuation. Cursor tokens are not issue IDs, stream offsets or authority to access another account. There is no client-provided page number, provider URL, raw JQL/GraphQL/search syntax, workspace hint or limit override.

Supported list adapters: GitHub, GitLab, Jira, Linear when configured with safe API origins. Literal provider-side title queries are initially supported by GitLab and Linear; GitHub and Jira return501 `unsupported` for a title query. No local page filter is presented as global provider search. This is an explicit remaining search-capability gap.

## Response

```json
{
  "schema":"issues.discovery.pages.v1",
  "source":"current_provider",
  "observed_at":"2026-11-09T00:00:00Z",
  "expires_at":"2026-11-09T00:10:00Z",
  "identity":{
    "host_id":"local","instance_id":"opaque",
    "tracker":"github","project":"org/repo","account":"work",
    "origin":"https://api.github.com","connector_epoch":"opaque"
  },
  "provider_project":"org/repo",
  "provider_project_id":"1234",
  "query":{"state":"open","title_query":null},
  "search_mode":"none",
  "rows":[{
    "number":"18446744073709551615",
    "title":"A current issue","state":"open",
    "updated_at":"2026-11-09T00:00:00Z","url":null,
    "title_truncated":false
  }],
  "pagination":{
    "mode":"live","order":"provider_order","page_size":25,
    "next_cursor":null,"end_observed":true,"truncated":false,"total":null
  }
}
```

Every row number is a canonical positive decimal UInt64 **string**, never a JSON number or Jira/Linear display key. Row state is `open` or `closed`. Title is at most4096 UTF8 bytes, with explicit `title_truncated`; updated_at is optional RFC3339; URL is optional HTTP(S), <=2048 bytes, without userinfo/query/fragment. No body/comment/labels are expanded. Maximum25 rows, no duplicate row numbers within one page. `search_mode` is exactly `none` or `provider_title_query`, matching query presence.

`identity.project` always echoes the original exact selection, while `provider_project` reports the connector-validated canonical project (notably Jira site/project form). To open an issue, pass the selected tracker, **identity.project**, account and origin with the row's decimal number into the existing current issue inspector. The inspector performs its own fresh exact account/project/issue validation. A list row is display/read context, not write authority.

Pagination is **live**, not a captured snapshot. Retrying a cursor may observe changed rows; provider insertion/reordering/deletion may cause cross-page omissions or repeats. Clients preserve returned order, retain previously displayed pages, and should qualify refresh explicitly. `end_observed` means the provider response demonstrated an end at that read; it is not a stable global completeness guarantee. `total` is always null. A full REST page may yield a conservative next cursor even when its next page will be empty. `truncated` means a bounded traversal limit stopped continuation; it must not be displayed as end/completion. Empty rows with a continuation are valid, including GitHub pages whose entries were all pull requests. GitHub PR objects are excluded only after validating returned repository identity.

`provider_project_id` is the verified provider repository/project/team object ID (1–256 noncontrol UTF8 bytes). Continuations bind this observed ID, so a later project-proof read that finds a replacement at the same path returns409. The proof and list are separate provider reads, not an atomic server snapshot; GitHub rows bind the exact repository URL, while GitLab/Jira/Linear rows additionally carry the proven project/team ID. No atomic no-replacement guarantee is claimed between the two reads. Native retains this ID across pages and resets it only on explicit refresh/new selection.

The HMAC cursor binds CP adapter instance, exact target, filters, connector epoch, provider project object ID and provider continuation. Its10minute lifetime begins at the first page and is not extended by paging. At most10000 page positions may be traversed. An epoch change requires explicit refresh. There is no snapshot cache and no fixed global total.

## Bounds and failure behavior

Two concurrent discovery reads per process, fail-fast admission;20second outer deadline includes adapter work. Each page does at most two bounded frozen-auth provider reads, each with10second transport timeout and2MiB body cap before JSON decoding. Requests retain the selected configured origin, reject redirects, do not retry provider calls, and recheck credential/connector generation. This is a payload/time bound, not a total parser RSS guarantee. CP validates adapter rows and caps serialized output at256KiB; overflow is a refused response, never a silently clipped row list. Token input16KiB/provider continuation4KiB. No extra per-row provider fetches.

Errors retain distinct outcomes:400 `invalid_target`/`invalid_cursor`;403 `forbidden`;404 `not_found`;409 `selection_changed`;410 `cursor_expired`;429 `rate_limited`;501 `unsupported`;502 `upstream_unavailable`;503 `account_unavailable`/`adapter_missing`/`busy`;504 `timeout`. Provider bodies, credentials and raw error URLs are not returned. No failure falls back to the unbounded runtime `issues.list` implementation or a different account.

Native UI should use the account catalog booleans, exact typed selection, thin25row pages, provider title search only when supported, and the current issue inspector for detail. Changing account/project/state/query starts a new read and invalidates a pending continuation. Preserve visible data on failure but never display it under a newly selected identity. Current issue observations remain distinct from recorded transcript trigger data. Source-only fixture validation and independent review are required before any runtime/parity claim.
