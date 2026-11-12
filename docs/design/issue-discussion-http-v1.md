# Issue discussion native boundary — frozen source DTO

Feature `issues.discussion.pages.v1` opts into the new current-issue inspector reader. Exact account rows also contain `discussion_supported`. Existing G1 `comments_supported` describes only its old route; do not combine both sets of comment rows. This is source implementation pending coordinated builds/tests and provider-shaped fixture validation, not live acceptance.

GET `/api/issues/:tracker/:number/discussion` with required `project`, `account`, `expected_origin`, optional `host=local`, optional `cursor`. `number` is a canonical nonzero decimal string. No cursor creates a fresh observation. Continue using the returned page cursor and the exact same target, endpoint and connection identity. GET `/api/issues/:tracker/:number/discussion/text` uses the same selectors and a required issued text cursor. No caller-selected URL/comment ID/offset enters either route.

Both endpoints return:

```json
{
  "schema_version": 1,
  "identity": {"host_id":"local","tracker":"github","project":"org/repo","account":"work","number":"7","origin":"https://api.github.com"},
  "source":"current_issue",
  "observation_id":"opaque",
  "observed_at":"2026-11-09T00:00:00Z",
  "expires_at":"2026-11-09T00:10:00Z",
  "consistency":"captured_pages_live_collection",
  "provider_identity":"64 lowercase hex characters"
}
```

`observed_at` is the particular provider page's capture time. Text chunks retain that same parent-page capture time. `expires_at` is fixed from observation creation. The identity digest binds internal provider project/issue and connector epoch; it is not a host-info token and is not a provider authorization promise.

Discussion adds `page`:

```json
{
 "id":"opaque-page-id","ordinal":0,"order":"oldest_first",
 "completeness":"page_only","duplicate_observations":0,
 "items":[{
   "id":"provider-comment-id","kind":"comment","author":"Author",
   "created_at":"2026-11-09T00:00:00Z","updated_at":null,
   "metadata_truncated":false,"parent_comment_id":null,
   "revision":"64 lowercase hex characters",
   "body":{"text":"Captured excerpt","offset":0,"byte_count":16,"total_bytes":16,
     "format":"markdown","source_format":"markdown","projection_complete":true,
     "unsupported_nodes":0,"unsupported_marks":0,"truncated":false,"next_cursor":null}
 }],"next_cursor":"opaque"
}
```

Field names and nullability above are literal; sample counts illustrate shape only. First ordinal0, each successful provider page increments by1. IDs are opaque bounded strings, never numeric JSON values. Items<=20. `kind` is comment/system; `order` oldest_first/provider_created_order. `completeness` page_only/end_observed. Empty items plus a continuation means continue reading, not empty/end. Repeated previously captured comment IDs with the same text revision are omitted and counted by `duplicate_observations`; changed revisions refuse the page409. Do not infer ancestry from `parent_comment_id`; it is optional recorded provider metadata, and Linear currently returns null rather than reconstructing reply trees.

Body `format` markdown/plain_text, `source_format` markdown/adf. Jira renders bounded ADF to plain text. `projection_complete=false` and omission counters mean that not all rich content/formatting is represented; byte completion alone must not override that qualification. Metadata truncation and text truncation are separate.

Text endpoint adds top-level `page_id`, `comment_id`, `revision`, and `body` with the same fields. It returns the next captured64KiB UTF-8 chunk; initial excerpts<=8KiB. Offsets/counts/total are bytes, not Swift character counts. Validate exact observation/provider digest/parent/comment/revision, expected byte offset, total and monotonic progress before appending. A nonterminal chunk has a cursor, terminal has null. All chunks come from one captured body; no reread of potentially edited provider content. No whole-thread request is needed.

Limits:512KiB encoded response; cursor<=128 bytes (current65), identity digest/revision64hex, comment IDs256B, author512B, timestamps128B, private vendor cursor2048B (never public). Upstream page2MiB before JSON parse, lightweight identity reads64KiB each (max4),20-second route timeout. Global two admitted readers. Eight observation slots,4MiB encoded retained payload each,256 frames,10-minute TTL. This is a retained encoded payload cap plus bounded staging/readers, not a total RSS guarantee. No LRU eviction: full cache returns capacity until observations expire. Cancellation before publication leaves original continuation reusable.

Errors are sanitized `{code,error}`:400 invalid_target/invalid_cursor;409 selection_changed/discussion_changed;410 observation_expired;413 observation_capacity;403 forbidden;404 not_found;429 rate_limited;501 unsupported;503 adapter_missing/account_unavailable/busy;502 upstream_unavailable/response_too_large;504 timeout. Preserve already displayed captured rows on failure. No old endpoint fallback and no automatic refresh/retry. An explicit refresh creates a separate observation.

Provider qualifications: GitHub's lightweight GraphQL identity supports positive signed32-bit issue numbers; larger numbers are unsupported. GitLab requires the configured API v4 dialect plus matching GraphQL endpoint. Linear uses documented Relay pagination and provider created-time order, not an assumed ascending order. Jira is Cloud REST v3/ADF. Account support indicates configured dialect support; it does not assert permission to read an issue. Cached replay validates the current local account/origin/epoch, but does not make a fresh provider authorization request.
