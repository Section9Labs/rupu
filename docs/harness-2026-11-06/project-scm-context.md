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
