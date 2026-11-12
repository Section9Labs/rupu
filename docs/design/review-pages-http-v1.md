# Review pages HTTP v1

Source candidate on a42e3bea; fixtures prepared, not executed. This is an additive operator HTTP read route. It neither extends the agent tool surface nor changes the existing review-detail route. Native integration and real-forge acceptance are separate work.

## Request

Feature: `project.scm_review_pages.v1`, advertised only by a RepoLister implementing the new port.

`GET /api/projects/:ws_id/scm/reviews/:number/pages`

| Query | Contract |
| --- | --- |
| host | Omit or literal `local`; no proxy/fallback. |
| expected_origin | Exact repository home URL from the owning checkout observation, max4096 bytes. |
| expected_account | Exact owning account selection, max256 bytes; a guard, never override. |
| expected_branch | Exact source branch, max1024 bytes. |
| expected_head | Exact recorded review head, max128 bytes. |
| expected_base | Optional exact base SHA from prior context, max128 bytes. Preserve presence/value throughout an observation. |
| section | Initial: `checks`, `threads`, `diff_files`. Continuations may additionally use `comments`, `text` only with issued cursor. |
| cursor | Opaque server capability, max8192 bytes. Repeat exact owner and section from its parent response. No parent/node/file URL/vendor cursor input. |

Unknown/duplicate query fields reject. Entire query is at most16KiB. Review number must be positive. Refresh means an initial request without cursor after refreshing checkout/review selection. Separate initial requests for each section create separate observations, not an atomic cross-section snapshot.

## Common successful envelope

- `ws_id`, `host_id:"local"`, `origin`, `account`, `number`, `branch`, `head_sha`, `base_sha` are mandatory exact owner echoes. `base_sha` retains the old context semantics: GitHub baseRefOid; GitLab diff_refs.start_sha (target revision). `merge_base_sha` is GitLab diff_refs.base_sha or null on GitHub.
- `section`, `scope`, `items` identify the returned typed section. A scope suffix on nested comments is an opaque recorded parent ID, never navigation authority.
- `cursor` replays this exact published page while the owner remains valid. `next_cursor` is the next page of this same section, or null. Never synthesize a cursor or parse its contents.
- `provider_exhausted` says the provider page stream is exhausted. It does not prove complete review activity or patch content. Internal captured-note/text continuations may coexist with provider exhaustion.
- `partial` is true when `omissions` is nonempty. Omissions are fixed strings, currently `pagination_unreported`, `provider_file_limit`, `provider_patch_missing`, `provider_diff_collapsed`, `provider_diff_limit`, `diff_limit_metadata_unreported`. Unknown omission codes must remain visible as incompleteness.
- `atomic:false`, `consistency:"head_bound_live"`, `first_observed_at`, `observed_at` qualify freshness. Published replay returns the original observed_at. Checks/comments may change on the same head before a later unpublished page is read.
- `provider_version` is the observed identity-endpoint version marker or null; `page_provider_version` is the separately bound section-endpoint marker or null. Neither null means a verified server release. The fixed connector API profile is bound internally and not a user-selected version override.

## Rows and nested continuations

All rows have a real vendor ID, or for diff files an exact captured path identity. These IDs do not grant filesystem/navigation authority.

| section | Row fields |
| --- | --- |
| checks | `id`, `name`, `state`, `url` (safe same-authority link or null). |
| threads | `id`, `path`, `line`, `resolved`, `comment_count`, `comments:[]`; `comments_cursor` when comments exist. Empty `comments` is an unloaded preview, never “no comments”; use comment_count and cursor. GitLab nullable location/resolution remains unknown. |
| comments | `id`, `author` nullable, `body`, `body_truncated`, `created_at`, `url`; optional `body_cursor`. |
| diff_files (GitHub) | `id`, `new_path`, `old_path`, `status`, `additions`, `deletions`, nullable `patch`, `patch_truncated`, `fragment:true`; optional `patch_cursor`. |
| diff_files (GitLab) | `id`, `new_path`, `old_path`, `new_file`, `deleted_file`, `renamed_file`, nullable `patch`, `patch_truncated`, nullable `collapsed`, nullable `too_large`, `fragment:true`; optional `patch_cursor`. |
| text | `id`, `text`, `byte_start`, `byte_end`, `total_bytes`, `sha256`, `fragment:true`; optional `text_cursor` on the row for the next exact fragment. |

Nested cursor mappings: `comments_cursor` → section comments; `body_cursor`, `patch_cursor`, `text_cursor` → section text. Text starts at byte0 when opening a body/patch cursor, replacing the short preview rather than appending it. Following text_cursor appends only when id/digest/total_bytes match and byte_start equals the preceding byte_end. All text is exact UTF-8; byte_end is exclusive. Text row id is the captured body/file identity. Its stable HTTP-v1 mapping is `comment:<comment-row.id>` for GitHub comment bodies, `note:<comment-row.id>` for GitLab note bodies, and `file:<diff-file-row.id>` for either provider’s patch. These are exact string concatenations; preserve the entire parent row id without parsing, trimming or case normalization (GitLab file row ids encode the old/new path pair). Before accepting the first text page, require its id to equal this expected parent-derived identity. Subsequent fragments must preserve that same id. Never concatenate fragments into a claim of an applicable complete patch; provider patch completeness is separate.

GH scopes: `head_commit_contexts`, `inline_review_threads`, `inline_thread_comments:<recorded-thread>`, `pull_request_files`. GL scopes: `head_pipeline_jobs`, `merge_request_discussions`, `captured_discussion_notes:<recorded-discussion>`, `merge_request_diff_files`. Text scope: `captured_text`. GitHub is inline review threads only, not the PR timeline, issue comments or all formal review activity. GL job scope excludes retried jobs, as before. Nested GL notes are the bounded exact discussion response; no invented independent vendor notes paging.

## Refusal and partial outcomes

The existing CP error envelope remains `{ "error": "fixed safe message" }`; no raw provider error body or credential is returned.

-400 malformed/duplicate/unknown selection or invalid section; non-local selectors use the existing local-only refusal.
-409 `selection_changed` on owner/head/base/account/credential/observed-version changes, conflicting duplicate identity or section mismatch. Keep already-shown data explicitly historical; offer Refresh, never auto-retarget/retry under new selection.
-410 `observation_expired` for unknown/tampered/evicted/expired cursor or restart; `observation_budget` when a retained/issued-cursor budget is reached. Offer Refresh. Do not claim a finished provider stream.
-501 adapter unavailable.503 shared readers or observation capacity exhausted.504 page deadline.502 provider/schema/byte limit unavailable; errors do not become empty successful pages. No automatic provider retry loop. Rate/auth failures are currently conservatively projected to502; retry-after hints are not exposed in this first slice.

## Bounded implementation and remaining limits

Two admitted review readers shared with old detail; one mutex per observation coalesces publication for identical cursors.25s outer request deadline includes local checks and forge reads. New provider page reads use64KiB before identity +2MiB page +64KiB after identity, streamed before JSON decode. New transports accept identity content encoding only, refuse redirects, preserve netflow observation, and make one review-API wire attempt per logical read. Existing owning OAuth refresh may run before an API request; its accepted replacement changes a nonsecret local epoch and invalidates a read crossing that replacement. This is not credential enrollment and does not inspect auth files.

Eight observations reserve8MiB each of encoded retained page/target data (64MiB total payload); at most16 published pages remain replayable per observation,512 issued nested/next targets,10-minute TTL. Older page tokens expire rather than silently refetching. Two bounded temporary decoded/staging copies and fixed owner/cache bookkeeping are additional transient/process memory; the64MiB figure is not total process RSS. Pages have at most40 checks or20 other rows,512KiB encoded output. Comment/patch previews are4096 UTF-8 bytes; exact text fragments are at most64KiB raw bytes to stay below the encoded output cap even with JSON escaping. Captured notes max2000 per provider page. Detectable duplicate IDs reject; this does not detect every same-head insertion/deletion in a live vendor collection.

Provider cursors never leave the server. Cursor capabilities use two independent ULIDs for160 random bits per observation and frame, stored with exact owner/section; they encode no accepted client claims and require no client-readable MAC payload. Cache loss invalidates all tokens. Registry selection is re-resolved from the current in-memory registry; external credential/config changes become observable when its owning credential source or registry adopts them, not through a new auth-file poll.

GitHub file pages stop at its3000-file ceiling, with an explicit omission when detected. GL diff-limit metadata absent on older versions stays unknown. Oversized individual provider objects, unknown API profiles, compressed responses, or missing full-page pagination metadata fail explicitly; this implementation does not download arbitrary blobs or widen to all repository activity. Current versions use guarded head/base tuples, not a separately fetched immutable GL diff-version snapshot. Existing old detail transport behavior remains unchanged.

Fixture evidence does not prove actual provider or native UI behavior. Warm tests/build, exact real-record read acceptance, and native visual review remain required.
