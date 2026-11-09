# Bounded current-issue discussion and captured comment text

Status: root-approved source implementation in progress; fixtures remain unrun. No builds or provider requests. Immutable source baseline `16cfcae7f8f4a5ccf9fe5b180c273f36ffe0b81a` (reviewed G1 + G3 composition). Source changes used an isolated checkout; the G4 worktree remained untouched.

## Existing limitation and chosen extension

G1 `api/issues_read.rs` uses a numeric page cursor and clips comment bodies at 8 KiB. Only GitHub implements `IssueConnector::operator_comments_page`; GitLab, Linear and Jira explicitly advertise unsupported. The legacy runtime `list_comments` contract is not a suitable replacement: it can walk many pages, and existing mutation/read transports have different retry policies. G3 already supplies the useful pattern: private provider continuation, retained bounded pages/text, process-wide admission and exact connector epoch checks.

Add a distinct opt-in `issues.discussion.pages.v1` capability and two GET routes. Preserve every existing G1 route, capability and old comment semantics. This avoids changing the numeric page contract to an opaque Linear cursor or silently changing old clients' thread ordering. Accounts gain an additive `discussion_supported` field for this new capability; old `comments_supported` continues to describe the old route. Native loads this section on explicit disclosure using its existing owning instance/account selection, not automatically alongside the old first comment page.

- `GET /api/issues/:tracker/:number/discussion?host=local&project=...&account=...&expected_origin=...&cursor=...`
- `GET /api/issues/:tracker/:number/discussion/text?host=local&project=...&account=...&expected_origin=...&cursor=...`

No cursor starts a new observation on the discussion route. Text requires an issued cursor. Caller never supplies provider page numbers, vendor cursors, URLs, comment IDs or byte offsets. Add matching operator MCP GET tools; no runtime/model grants or writes.

## Authority and transport

Reuse G1 exact explicit account selection and configured-origin equality, including Jira configured site/port/context path before attaching credentials. Add a narrow connector discussion port rather than broadening runtime IssueConnector defaults. Its immutable identity includes tracker/account/configured API profile, random connector lifetime plus credential replacement epoch, exact provider project/team ID, issue ID and selected issue identifier. Returned references must agree with selected issue/project; aliases/moved resources fail closed. Credential values never enter tokens or hashes. GitHub/GitLab can expose the existing G3 client lifetime/epoch primitives. Linear/Jira have immutable credentials in their connector instances; a random instance lifetime suffices until a credential-replacement mechanism exists, and replacing a connector invalidates the observation.

Each new provider page makes bounded lightweight identity reads before and after one discussion request: max four64 KiB identity responses plus one2 MiB page decoded body, and at most five logical API requests (two identity requests per side for team/project resolution, plus page). Exact request count is provider-specific and documented. Identity queries must not download the issue body. The new dedicated operator discussion transport disables redirects and reqwest automatic retries explicitly, preserves netflow, bounds response headers to128 KiB and decoded page body to2 MiB before JSON parse, and sanitizes errors. It does not enter existing `with_retry` or GitLab `send_authed` 401 replay. Existing token-source preemptive refresh remains owned by that source; if its epoch changes during the operation, reject publication and let the user refresh. No retries or behavior changes in existing G1/runtime transports.

Cached page/text replay performs exact local connector/account/profile/epoch validation, with no provider request. This is a retained observation already delivered to the client, not a fresh provider authorization check. Expiry/eviction/restart ends access; providers changing permissions after capture do not retroactively make retained bytes a new current observation.

## Provider page projections

Request at most20 rows, preserve provider order, reject duplicate IDs within a page. Missing required parent identity or malformed pagination is an unavailable error, never empty success.

- **GitHub:** issue comments endpoint under exact repository + issue number. Verify each `issue_url`, comment ID and canonical API location against the verified parent; use existing repository/issue identity checks strengthened with immutable IDs. Numeric continuation is private. A full page requires valid next pagination evidence or conservatively offers the next sequential page without claiming exhaustion; a short page is `end_observed`. Do not follow Link URLs.
- **GitLab:** `/projects/<verified numeric project ID>/issues/<iid>/notes`, `order_by=created_at&sort=asc`. Verify `noteable_type=Issue`, noteable ID and iid against verified issue. Preserve system notes with `kind=system` instead of silently filtering them. Explicit X-Next-Page/Link pagination is validated against the same request authority/path/filter; never followed directly. Missing pagination on a full page is unavailable, not exhausted. Short page may establish `end_observed`.
- **Linear:** nested exact issue `comments(first:20,after:...)` with Relay pageInfo. Resolve selected team key/ID and exact issue identifier before page; verify returned issue ID/team/identifier and each comment's parent issue where available. Vendor cursor max2048 bytes; hasNextPage requires a nonempty changing endCursor. The documented ordering field is createdAt; do not claim oldest-first without an explicit supported sort direction. Wire order is `provider_created_order`. Preserve recorded parent-comment ID only if the provider schema supplies it; do not fabricate thread ancestry or claim exhaustive reply-tree reconstruction.
- **Jira Cloud v3:** `/rest/api/3/issue/<verified issue ID>/comment?startAt=...&maxResults=20&orderBy=created`. Verify exact issue key and ID first and comment `self` path against configured origin/context path, exact numeric issue ID and comment ID. Validate startAt echo, bounded count, total and forward progression; total is observed mutable metadata, not a snapshot count. Preserve author display name, created/updated, optional visibility metadata without inventing authorization. Body is bounded ADF projection to plain text, explicitly `format=plain_text`, `source_format=adf`, with unsupported node/mark counts and `projection_complete=false` where content cannot be represented. Do not label rendered ADF as original Markdown. Retain all projected text within bounds; rich media is a limitation, not a silently complete body.

Vendor schema particulars will be checked against primary documentation/schema during implementation. If any required exact parent evidence is absent from a supported dialect, that dialect returns unavailable rather than weakening ownership.

## Additive wire shape

All responses echo the existing exact `identity:{host_id,tracker,project,account,number,origin}` with decimal-string number. New fields: `schema_version:1`, `source:current_issue`, `observation_id`, `observed_at`, `expires_at`, `consistency:captured_pages_live_collection`, and an opaque provider-identity digest. The digest is not a new host-info ID.

Discussion page:

```
page: {
  id, ordinal, order, completeness: page_only|end_observed,
  items: [{ id, kind:comment|system, author:null|string,
    created_at, updated_at:null|string, metadata_truncated,
    parent_comment_id:null|string,
    body:{text,offset:0,byte_count,total_bytes,format,source_format,
      projection_complete,unsupported_nodes,unsupported_marks,
      truncated,next_cursor}, revision }],
  next_cursor:null|string
}
```

Initial excerpt up to8 KiB UTF-8; full projected comment text remains privately captured. `revision` hashes full captured projected text and exact comment identity, not all provider metadata. A body cursor binds exact observation/target/connector epoch/page/comment/revision/UTF-8 byte offset. Text response echoes `page_id`, `comment_id`, `revision` and `body` with next64 KiB chunk, exact byte position/total, and its next cursor. Text continuation reads those captured bytes, not a future edited comment. Every nonterminal chunk advances at a UTF-8 boundary. Missing/deleted/edited comments cannot substitute bytes behind an existing cursor.

Provider pages remain a live collection, not a frozen entire thread. Replayed issued page tokens return the same page. A newly requested next provider page can reflect edits/additions/deletions. Repeated IDs with identical captured revision are reported as duplicate observations and omitted from new display rows; a repeated ID with changed revision refuses that page as `discussion_changed`409 and asks for explicit refresh. An empty page with a valid continuation is allowed and never presented as completed. `end_observed` means provider exhaustion at that request, not future completeness.

## Bounded cache, cancellation and errors

Reuse G3 private frame/captured-text mechanics where extraction is small; avoid coupling issue identity to SCM branch/head DTOs. Prefer a separate small typed cache using the same reviewed pattern over a broad generic refactor.

Two process-wide admitted readers, no waiter queue;20-second overall deadline. Upstream page2 MiB, each identity64 KiB, header128 KiB, wire response512 KiB.20 comments/page, excerpt8 KiB, text chunk64 KiB, comment ID256 bytes, metadata same bounded limits as G1. Jira ADF traversal depth<=64 and nodes<=10000; overbudget fails visibly. No automatic pagination or whole-thread capture. Retain issued provider cursors in the bounded observation; repeated nonadjacent tokens are a protocol error rather than progress.

Eight observation slots, each reserves4 MiB encoded retained payload and max256 frames; total retained serialized cache ceiling32 MiB. TTL10 minutes, monotonic time; opaque160-bit random frame tokens <=128 bytes, private scope-bound lookup. Encoded retained bytes include text, page receipts and pending provider continuation. A page exceeding remaining budget returns413 `observation_capacity`; it is not silently clipped or marked complete. Expired/evicted observation410 `observation_expired`; foreign cursor400; epoch/parent identity drift409; busy503. Reuse sanitized G1 codes for provider403/404/429/502/504. No unknown error becomes end_observed.

Build each page and its child text tokens in a private draft; publish atomically under a short cache lock only after provider identity checks, bounds and serialization succeed. Cancellation publishes nothing and consumes no continuation. Avoid holding global locks across network calls; per-observation duplicate requests are serialized or promptly busy, not double-dispatched. A successful cached token replay causes no new provider request.

## File and test plan after approval

- New SCM discussion DTO/transport helper; narrow additive default trait methods; four provider projections alongside existing issue connectors. Existing legacy methods unchanged.
- CLI `cp_issues` exact resolution + discussion bridge/epoch; CP issue_reader additive port; new `api/issues_discussion.rs` plus bounded cache module and AppState capability wiring.
- Operator MCP route/schema/name inventory, explicit no-runtime-grant assertions; no HTTP exclusion shortcut.
- Disposable provider-shaped fixtures for all four identity scopes, foreign comment parent, Jira context path/ADF bounds, Linear cursor progression, GitLab system notes/headers, GitHub full/short pages; no actual account reads.
- Transport fixtures: redirect refusal, automatic retry disabled,401 single dispatch, no credential leak, oversized/error bodies, sanitized GraphQL failures, epoch change before publication.
- CP fixtures: all owner dimensions, foreign token, expiry/eviction/capacity, duplicate/changed comments, UTF-8 long-body full reassembly, cross-comment cursor refusal, immutable replay, canceled pending page/no token advancement, zero rows plus continuation, exact process-wide capacity/admission.
- Existing G1 fixtures unchanged and rerun for compatibility; independent source review before coordinated warm tests. Native then adds explicit discussion and text loading in the existing issue inspector, keeping recorded trigger context distinct.

## Primary references checked for proposal

GitLab notes API documents issue notes and noteable identity fields: https://docs.gitlab.com/api/notes/
Linear documents Relay first/after and pageInfo, with createdAt ordering: https://linear.app/developers/pagination
Jira Cloud v3 comment API documents issue-qualified comment list with pagination and ADF bodies: https://developer.atlassian.com/cloud/jira/platform/rest/v3/api-group-issue-comments/
