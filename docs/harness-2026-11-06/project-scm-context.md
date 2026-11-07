# Project checkout context

`GET /api/projects/:ws_id/scm?host=local` is advertised as `project.scm_context.v1`.
It reads the registered checkout on the owning control-plane instance. Remote host
selectors are refused; it does not inspect a similarly named path on the client Mac.

The response preserves `ws_id`, `host_id`, `source: local_checkout`, `checkout_path`,
`observed_at`, and `state` (`ready`, `not_repository`, or `unavailable`). A ready
checkout contains current `branch`, `head`, `detached`, a credential-free recognized
`repository` identity, and `changes` with separate `index_status` and
`worktree_status`. Staged renames preserve `original_path`; untracked directories
are summarized rather than recursively enumerated. Non-UTF8 display paths set
`path_lossy` and preserve original bytes in `path_bytes_base64`; clients must not
use the lossy display value as a filesystem action target. An unborn branch has no HEAD.

`working_diff` compares index to worktree; `staged_diff` compares HEAD to index.
Each contains `text` and `truncated`. Untracked file contents are not read into the
diff. Binary/large-file changes may be summarized by libgit2. Neither diff is a
remote PR/MR diff. `pull_request.state` is `not_queried`: this endpoint makes no
remote authorization or review-state claim. Unknown forges have no guessed web URL.

The implementation uses libgit2, not shell commands or Git subprocesses. It never
runs hooks, external diff tools, clean/process filters, fsmonitor commands, network
requests or credential helpers. Status is read with index updates disabled.

Limits: at most 200 changed entries and 256 KiB of UTF-8 text per diff. The 10-second
HTTP deadline bounds caller wait. At most two scans run concurrently. A timed-out
blocking scan retains its permit until it exits. These limits do **not** establish
bounded repository scan cost: libgit2 may traverse the complete index/worktree.
`scan_limit` exposes that distinction. `atomic: false` warns consumers that branch,
status and the two diffs are sequential observations while other tools may modify
the checkout; refresh before consequential actions. No write operation is offered.

Tests use disposable real Git repositories and cover staged/working separation,
untracked content exclusion, index-byte preservation, malicious configured command
nonexecution, origin credential stripping, staged rename provenance, response
limits, nonrepository behavior, registration and local-host ownership checks.
No provider or live SCM service is contacted by these tests.

## Explicit remote review lookup

`include_review=true` additionally asks the existing CP-owned SCM registry for the
checkout origin repository's current branch reviews. The default request performs
no remote query. Account selection reuses registered repository/path rules; it
never chooses another instance's credentials or resolves ambiguous accounts by
picking the first. A separate 15-second deadline bounds this optional operation.

`pull_request` becomes `{state: queried, account, reviews, truncated, scope,
queried_at, freshness}` on success. Every review retains its title/number/URL,
state, draft flag, head/base branch and SHA, and nullable `checks_state`. Missing
checks are not success. GitHub uses a fixed GraphQL query with variables, six
results and one status-check rollup per result. GitLab queries a source-branch
page of six, then at most five detail records for diff refs and the head pipeline.
Only five rows are displayed; `truncated` warns about remaining candidates. A
same-named branch in another fork is excluded. This scope does not discover a
fork's upstream destination repository. Connector cache policy still applies.

Read failures become `state: unavailable` with a generic reason; upstream error
bodies and credential-bearing URLs are never forwarded. Local checkout status and
diffs remain available when remote reviews fail. No review/comment/branch/merge
mutation is offered. These remote reviews do not alter the meaning of either
local diff, and the sequential checkout/remote observations are not atomic.

The vendor SHA fields have different meanings. GitHub `baseRefOid` is the target
ref commit. GitLab `diff_refs.start_sha` is the target commit recorded for the diff
version, so it maps to `base_sha`; `diff_refs.base_sha` is separately retained as
`merge_base_sha`. It is not mislabeled as the current target tip. The GitLab fields
may be temporarily empty while a new MR's diff is computed. See the official
[GitLab merge-request contract](https://docs.gitlab.com/api/merge_requests/) and
[GitHub GraphQL reference](https://docs.github.com/en/graphql/reference/git).

Self-hosted GitHub/GitLab origin hosts are recognized only when the owning
instance's global SCM configuration declares the matching platform/base URL.
Unknown hosts remain unclassified; an invalid configuration does not turn an
unknown origin into a guessed public forge or borrow another account.

After account selection, the adapter also compares the checkout origin's web host
with the selected connector's authority. Matching platform/owner alone cannot send
a github.com checkout query to an enterprise instance, or vice versa. Mismatch
fails before any remote request; it never retries using a different account.

## Verification

Six focused CP tests pass, including explicit remote-query opt-in. Four CLI
repository-adapter tests pass, including local HTTP fixtures for GitHub GraphQL
and GitLab branch/MR detail routing, correct account attribution, fork exclusion
and refusal of a mismatched origin authority before network. The HTTP fixtures
require loopback listener permission; initial sandbox listener failures were
rerun successfully using the exact compiled test binary with loopback access.
No real forge credentials or live repository API were used. Native acceptance
against the integrated control plane remains a separate check.

Commands use `CARGO_BUILD_JOBS=4 CARGO_PROFILE_DEV_DEBUG=0
CARGO_PROFILE_TEST_DEBUG=0` and `--no-default-features`. The CLI test link emits an
existing large `__eh_frame` linker warning; test execution succeeds. Changed Rust
files pass rustfmt; `git diff --check` passes. Whole-workspace rustfmt has unrelated
pre-existing drift and was not rewritten by this change.
