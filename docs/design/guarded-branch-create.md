# Guarded repository branch creation

Source-only extension of the existing operator ForgeWrite flow. No new route,
MCP tool, execution permission, checkout operation, or CI dispatch is added.

`branch.create` is offered by configured GitHub/GitLab forge adapters only when
the owning repository adapter exists. It requires a registered local workspace,
exact provider/account/API origin/project and no issue/review number. `input.head`
is the new branch name; `input.base` is an existing remote branch name. No title,
body, labels, state, draft, or choice fields are allowed. Optional expectations
are `provider_project_id` and `base_sha`. Names exclude full refs, HEAD, revision
expressions, empty/hidden/lock components and whitespace; no rewriting occurs.

Preparation proves the repository, reads the exact base branch and hexadecimal
40/64-character commit, and requires an explicit 404 for the destination branch.
An existing destination returns `selection_changed`; authorization, transport,
malformed response and other failures never imply absence. The captured review
records `provider_project_id`, `base_branch`, `base_sha`, `new_branch` and
`new_branch_absent: true`. The CLI appends the existing checkout identity, origin,
branch and head authority even when a different remote base was selected.

Execution retains the existing local owner/instance/credential generation and
intent digest checks, re-reads both branches, and compares the captured state.
It sends exactly one mutation using the captured SHA: GitHub POST git/refs or
GitLab POST repository/branches. It does not use the older retrying repository
connector path. Providers may change after preflight: no atomic CAS is claimed.
A competing creation may cause rejection. The request never updates an existing
reference or changes the local checkout.

201 acknowledgement must return the exact destination and captured commit.
GitHub additionally proves its API reference URL and commit type. GitLab's
branch DTO lacks a project ID; repository ownership rests on the frozen request
endpoint and preflight repository proof, while returned name/commit are checked.
Malformed, partial, mismatched or contradictory acknowledgements remain unknown.
The canonical result has effect, verification, resource_id (`refs/heads/<name>`),
branch, head_sha, and provider_project_id. Recovery reads the saved operation;
it does not dispatch another mutation. The uncertainty dedupe key ignores the
base branch, so changing a base cannot repeat uncertain creation of the same
provider repository/destination branch.

## Validation qualification

No builds, tests, provider requests, live writes, services or active worktree
changes were performed in this source lane. Rustfmt parsing and diff whitespace
checks are source checks only. Eight new backend fixture functions cover safe
names/provider scope, both provider round trips, changed base/destination,
non-404 absence and provider mismatch, exact acknowledgement, unknown once,
checkout authority, and CP destination uncertainty key. Existing guarded-write
store/route/credential fixtures must also run in the root-owned warm slot:

```
CARGO_BUILD_JOBS=2 cargo test -p rupu-scm --lib forge_write -- --test-threads=2
CARGO_BUILD_JOBS=2 cargo test -p rupu-cli --lib cp_forge_writer -- --test-threads=2
CARGO_BUILD_JOBS=2 cargo test -p rupu-cp --lib forge_write -- --test-threads=2
CARGO_BUILD_JOBS=2 cargo test -p rupu-cp --test it mcp_routes -- --test-threads=2
```

Use default features. No model tools/registry counts changed. The native action
enum/catalog reader must be delivered with this capability extension: older
native versions reject unknown action values in the capability response.

Downgrade qualification: an older native reader also refuses persisted branch.create
operation journals. Preserve those journals and use the companion/newer client for
recovery; do not erase uncertain operations to make a downgrade appear compatible.
