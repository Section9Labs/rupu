# G3: bounded review continuation — design for approval

Status: approved for isolated implementation; source candidate and fixtures prepared, validation pending. Source base: a42e3beaa5736998fabcf9513976545044e326de. Isolated branch: codex/review-continuation-design. This document does not authorize forge writes, credential enrollment, live acceptance, or a service restart.

The G3 coverage matrix identifies this gap. Its gap is continuation of an already reachable bounded review inspector. GitHub discussion scope remains inline review threads, not all PR activity. Existing account/head/origin constraints remain authoritative.

## Source findings

- `crates/rupu-cli/src/cp_review_detail.rs:8–13` caps projected checks at40, threads20, comments/thread5, diff256KiB. GraphQL requests one extra item and retains only hasNextPage, not endCursor. GitLab requests first pages and loses response pagination headers. This is output truncation, not a continuation contract.
- `crates/rupu-cp/src/api/project_scm.rs:94` re-observes the registered local checkout, compares canonical origin and current branch, then resolves the owning account through RepoLister. ReviewSelection is a precondition, never an account override. Two review readers and a25s outer deadline already exist.
- `crates/rupu-scm/src/connectors/github/client.rs:225` calls response.json without a body ceiling. `github/repo.rs:267` reads the complete diff string before projection. `gitlab/client.rs:240` returns body-only JSON, discarding Link/X-Next-Page; its error-body reads are unbounded too. New continuation must not reuse these paths and claim byte-bounded reads.
- `Registry` builds account clients and rules once; account name alone does not identify connector configuration lifetime. GitLab TokenSource can replace OAuth credentials behind a client. No nonsecret credential-generation contract currently exists for page binding.
- Current GitHub head/base checks protect optional number-addressed diff reads. GitLab verifies before/after MR identity and base/start/head. No provider snapshot spans all future check/comment pages.

## Proposed scope and provider capabilities

Add opt-in `project.scm_review_pages.v1`; keep the existing bounded detail API unchanged for older clients. Support only sections with exact owner qualification and a supported connector profile. Capability advertisement requires the new adapter, not merely any RepoLister. Initial observations additionally advertise each section's actual availability; unsupported provider schemas never become empty sections.

| Section | GitHub | GitLab |
| --- | --- | --- |
| Checks | Head-commit statusCheckRollup.contexts, explicit first/after + stable node identity; retain both CheckRun and StatusContext types | Jobs of the captured head pipeline ID whose SHA equals captured MR head; preserve current jobs scope, do not include other pipelines or change retried-job policy |
| Discussion | PR reviewThreads connection; comments use their own cursor on the exact already-admitted thread and reverify its repository/PR parent | MR discussions page with exact project numeric ID/MR IID; notes inside one returned discussion have no independently proven paginated continuation in the current connector |
| Comment bodies | Further chunks of one bounded captured comment value, tied to its exact comment ID and content digest | Same; if the single discussion response exceeds input bounds, report unavailable instead of scanning all MR notes to guess membership |
| Diff | PR files pages, captured base/head and file identity; bounded patch fragments where provider supplies them | MR diffs pages under captured base/start/head tuple; use immutable diff-version identity where the deployed supported API supplies it |

GitHub documents cursor/after traversal with pageInfo and1–100 requested elements. Its review-thread schema includes comments and exact repository/PR parent fields. This supports guarded independent thread/comment continuation. [GraphQL pagination](https://docs.github.com/en/graphql/guides/using-pagination-in-the-graphql-api), [review-thread schema](https://docs.github.com/en/graphql/reference/pulls#pullrequestreviewthread).

GitHub's file-list endpoint is paginated but capped at3000 files. A finished vendor page walk therefore cannot mean all files when the PR exceeds that limit; missing patch text also remains an explicit omission. [Pull-request files API](https://docs.github.com/en/rest/pulls/pulls#list-pull-requests-files).

GitLab documents pagination for discussion lists and a separate exact-discussion read; the latter does not expose a notes paging contract. Its MR diffs API accepts page/per_page and newer schemas expose collapsed/too_large states. We must qualify older/self-managed schemas and retain vendor-imposed omissions. [Discussions API](https://docs.gitlab.com/api/discussions/), [MR diffs API](https://docs.gitlab.com/api/merge_requests/#list-merge-request-diffs), [diff limits](https://docs.gitlab.com/administration/diff_limits/).

These vendor facts do not justify synthesizing absent check IDs, thread membership, diff contents, pagination headers, or total counts. No fallback to repository-wide commits, all notes, raw URLs, local checkout diff, fetched blobs, git fetch/clone, or another account is allowed. Full PR issue comments/formal review activity remain outside this GitHub section's scope.

## Read API and immutable owner binding

Proposed additive GET route:

`/api/projects/:ws_id/scm/reviews/:number/pages?host=local&section=checks|threads|comments|diff_files|text&...`

Initial read requires the existing expected_origin/account/branch/head selection plus the known base SHA when available. Subsequent reads repeat the same owner preconditions and carry only the server-issued cursor; section must agree. Nested sections accept only capabilities issued by an admitted parent page, not arbitrary caller-supplied thread IDs, file paths, vendor URLs or GraphQL node IDs. Duplicate/unknown query parameters reject. Cursor length maximum8KiB.

Binding includes CP instance/registered project identity and canonical checkout; full normalized web origin (scheme/authority including port/path); configured API authority/profile; platform; resolved account ID; registry/connector lifetime; credential generation; numeric PR/MR identity and immutable vendor repository/project ID; source branch; head/base (and GitLab start/diff-version when available); exact section and nested parent identity. Account resolution still uses owning-cwd rules on each read. The same account label with a different connector or credential generation is a new observation.

Add a nonsecret random generation on connector construction, and a monotonic/random opaque credential epoch owned by the existing token source whenever credentials are replaced/refreshed. Never derive it from a secret, expose tokens, inspect provider auth files, or introduce an identity/login probe. The new reader must refuse continuation if it cannot establish a stable generation. Normal auth refresh remains within the existing owning auth mechanism; a refresh across a page read invalidates that observation rather than silently combining identities.

Provider profile is explicit: fixed supported GraphQL projection/API dialect and pinned supported REST API version where available. Capture version/schema markers actually returned by the provider; change or malformed required fields invalidates the observation. An unreported server version is recorded as unreported, not invented. We cannot detect an invisible provider upgrade whose responses remain identical; the guarantee is the declared projection contract, observed version markers and exact owner/head tuple.

Check owner/configuration before and after each upstream page fetch. Identity reads bypass current connector caches or revalidate ETags with the provider; a cached body is not a fresh head check. If head/base/branch/project/origin/account/generation/profile changes, return409 selection_changed with no rows from that request. Previously displayed pages remain explicitly historical until the user refreshes. No automatic continuation under the replacement owner.

## Continuation and freshness semantics

This is a **head-bound live observation**, not a transactionally frozen review snapshot. Checks can finish, comments can be edited/deleted, and threads can be resolved without changing the PR head. Emit `atomic:false`, `consistency:head_bound_live`, per-page observed_at, first_observed_at and explicit section scope. Compare available total/count/order markers and reject detectable pagination inconsistencies; deduplicate by genuine vendor IDs, never row positions. A duplicate ID with conflicting content is a refresh-required change, not a second item or silently overwritten history.

Cache each published CP page and its nested captured text immutably. Retrying an issued cursor returns the same page if the owner/generation still qualifies. Do not repeatedly fetch the same vendor page after transport loss. The next CP token identifies an exact server observation/page slot and includes a MAC binding the owner/section; vendor cursors and next URLs remain server-side. Concurrent identical reads coalesce; the observation frontier advances only after a bounded valid page is published.

When output limits split a vendor page, retain its bounded remainder and issue an internal slice cursor before advancing the vendor cursor. This prevents skipped rows caused by projecting40 of41 and then using the41st item's endCursor. GitHub should request the actual desired page size and use pageInfo rather than an extra item when possible. GitLab Link/X-Next-Page is parsed as data, checked for same authority/path/owner and a forward bounded page; reconstruct the fixed endpoint locally. Never follow a supplied URL with credentials or trust a client-provided vendor cursor.

`next_cursor:null` means no further page is offered for this observation, not universal completeness. Report `provider_exhausted`, `partial`, and typed omissions separately: provider_file_limit, provider_patch_missing, provider_diff_limit, input_byte_limit, observation_budget, unsupported_nested_paging, expired. Do not return an empty successful page after a failed provider read.

Refresh explicitly creates a new observation from current repository/review context and discards old continuation tokens. It may include new data and changed status; it does not append new-head rows to the old result. Restart, expiry or cache eviction returns410 observation_expired; native offers Refresh. No durable data cache or mutation journal is needed for this GET-only facility.

## Diff and long text handling

Do not fake byte offsets into a mutable forge diff stream. Fetch bounded file pages; each retained patch/comment body is immutable within the observation and gets a content digest plus UTF-8 byte-range chunk cursor. Keep old/new file identity, rename/status, captured base/head and vendor file identifier when supplied. Render fragments as fragments, not complete/applicable patches. Chunk only on valid UTF-8 boundaries, preferably lines; an oversized single line gets explicit continuation without altering its bytes.

A provider-omitted/binary/oversized patch cannot be reconstructed from unrelated sources. Label only the reason the provider positively reports; absent patch alone does not prove binary. GitLab collapsed and too_large remain distinct, and an unsupported per-file expansion route stays unavailable. An exact thread's already-returned notes may be CP-paged within the bounded captured object; that does not invent a vendor notes cursor. A too-large single thread remains partially unavailable with its forge link.

## Proposed budgets

- Existing two concurrent review reads per CP, shared by old detail and continuation; one in-flight fetch per observation. No unbounded wait queue or prefetch fan-out. Cancellation drops upstream work; any bounded coalesced task retains its permit until it stops, including after a caller disconnects.
-25s total page deadline including permits, identity reads and provider reads; at most3 logical provider reads (before/page/after), at most6 wire attempts including existing authenticated replacement. No automatic transient/rate-limit retry loop in the new reader; surface sanitized retry_after.
-2MiB decoded/decompressed provider body per response and4MiB total input per CP page;128KiB max headers/error body accounting, errors exposed only through fixed safe codes. Enforce during streaming before JSON/text materialization, irrespective of Content-Length. Abort/drop on excess/cancellation.
- CP output at most512KiB; at most40 checks,20 thread headers,20 comments,20 file headers,256KiB text per page. Adaptive one-file/one-thread reads may be offered only through the same exact endpoint/profile; no hidden scan.
- At most32 active observations and64MiB retained memory per CP,8 observations per account;10-minute TTL. At most16 retained published pages per observation,8MiB each observation, and10,000 identity entries/512 vendor pages per section. Old page eviction is explicit410, not a refetch silently replacing its contents. Budget limits are reported omissions and never called provider exhaustion.
- Track actual retained page/text/cursor/identity bytes and all nested rows. Count limits alone are not byte limits. The existing old detail endpoint keeps its current qualification until separately migrated; this feature does not retroactively make that implementation input-bounded.

## Implementation shape after approval

1. Add typed RepoLister review-page capability and safe errors; add scoped observation/cache module in CP and shared reader semaphore. Existing detail route/output stays compatible.
2. Add narrowly scoped bounded-read primitives to GitHub/GitLab clients that preserve safe pagination/version headers and support fresh identity checks; extend extras with fixed queries/endpoints. Add connector/credential epoch support without changing account-selection authority or exposing secrets.
3. Implement GitHub checks/thread/comment pages and bounded file pages; implement GitLab captured head-pipeline jobs/discussions/diff-file pages only for supported response profiles. Nested notes/text become internal pages when fully captured within bounds; otherwise omit explicitly.
4. Add capability/scope/omission DTOs; only then coordinate the native SCM owner. Native adds small Load More actions in existing sections, independent nested comment continuation, and Refresh after changed/expired observations. Existing forge links and scope qualifiers remain. No new dashboard or write action.

## Required fixtures and acceptance

Disposable connector fixtures: page2 exact IDs and no lost extra row; nested comments page2 bound to thread+PR; foreign/reused node IDs; changed head/base/branch/repository/account/credential generation/API profile; same-head comment edits/conflicting duplicate IDs; malformed/cyclic/cross-origin vendor links; number/scope tampering; wrong project after registry replacement; GitHub3000-file omission; GitLab collapsed/too_large/missing pagination/unsupported schema; body overrun before decode; missing/binary patch not fabricated; UTF-8 chunk reconstruction; quotas/expiry/restart; same-cursor concurrency coalescing and cancellation;503/rate-limit without empty success. Read-only fixtures assert no mutation endpoint, credential enrollment or cross-account fallback is invoked.

Separate later acceptance must use actual authorized large PR/MR records through the native production route, both themes/widths, then a deliberately changed owner/head and a constrained provider. Source fixtures are not live/provider proof. A full G3 closure cannot promise every byte of every arbitrarily large review: vendor omissions, unsupported legacy profiles and explicit local budgets remain visible. The achievable first milestone is continuation through all available pages in the declared scope without silently widening it.

## Approved implementation refinement

Root approved this additive GET-only design. The source candidate uses a stricter eight-observation bound, encoded retained data, and opaque160-bit random server-map capabilities rather than decoded/MAC claims. Comment bodies use64KiB exact text fragments. Rate-limit retry hints and immutable GitLab diff-version expansion are not in the first slice; errors/omissions remain explicit. See [Review pages HTTP v1](review-pages-http-v1.md) for the exact DTO, scope and validation limits. No builds or real-forge acceptance have run in this isolated task.
