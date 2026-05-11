# SCM & issue trackers

rupu integrates with SCM (source-code management) platforms and issue trackers
through a single embedded MCP server. Agents call typed tools (`scm.repos.list`,
`scm.prs.diff`, `issues.get`, ...) regardless of which platform the call resolves
to. Per-platform connectors handle vendor-specific quirks (GitLab MR vs GitHub
PR, nested namespaces, rate-limit headers).

## At a glance

| Capability                          | GitHub | GitLab | Linear | Jira |
|-------------------------------------|:------:|:------:|:------:|:----:|
| Repos (list, get, branches)         |   ✅   |   ✅   |   —    |   —  |
| PRs / MRs (read, comment, create)   |   ✅   |   ✅   |   —    |   —  |
| Issues (read, comment, create, transition) | ✅ |   ✅   |   —    |   —  |
| Workflow / pipeline trigger         |   ✅   |   ✅   |   —    |   —  |
| `clone_to` (local checkout)         |   ✅   |   ✅   |   —    |   —  |
| File read by ref                    |   ✅   |   ✅   |   —    |   —  |
| API surface                         |  REST  |  REST  |   —    |   —  |

Linear and Jira currently participate through **native trigger sources** (webhook + polling), not through the MCP `issues.*` tool surface yet.

## Auth

> New project? Run `rupu init --with-samples` to seed `.rupu/agents/scm-pr-review.md` and the rest of the curated templates.

`rupu auth login --provider <github|gitlab|linear|jira> --mode <api-key|sso>` stores tokens
in the OS keychain. Same flow as Slice B-1's LLM-provider auth; `rupu auth status`
picks up SCM rows automatically.

GitHub uses a device-code SSO flow; GitLab uses browser-callback PKCE. Required
scopes:

| Platform | Scopes                                              |
|----------|-----------------------------------------------------|
| GitHub   | `read:user`, `repo`, `workflow`, `gist`, `read:org` |
| GitLab   | `api`, `read_user`, `read_repository`, `write_repository` |

Linear and Jira currently use API-key mode only:

| Platform | Current use |
|----------|-------------|
| Linear   | native trigger polling / webhook normalization |
| Jira     | native trigger polling / webhook normalization |

For Jira Cloud polling, store the credential as `<email>:<api_token>` in one API-key secret.

## Target syntax

The optional positional arg on `rupu run` and `rupu workflow run`:

| Form                                | Means                          |
|-------------------------------------|--------------------------------|
| `github:owner/repo`                 | repo (working tree)            |
| `github:owner/repo#42`              | PR 42                          |
| `github:owner/repo/issues/123`      | issue 123                      |
| `gitlab:group/project`              | repo (working tree)            |
| `gitlab:group/sub/project!7`        | MR 7 (gitlab uses `!` not `#`) |
| `gitlab:group/project/issues/9`     | issue 9                        |

When the target is a Repo or PR (not an Issue), rupu clones the repo into a
tempdir and runs the agent there. Issue targets don't trigger a clone; the
agent's read tools work without a checkout.

## MCP tool catalog

All 17 tools in the unified surface. Each accepts an optional `platform?` (or
`tracker?`) that falls back to `[scm.default]` / `[issues.default]` from config
when omitted.

| Tool                          | Kind  | Description |
|-------------------------------|-------|-------------|
| `scm.repos.list`              | Read  | List authenticated user's repos on a platform |
| `scm.repos.get`               | Read  | Fetch a single repo (default branch, clone URLs, visibility) |
| `scm.branches.list`           | Read  | List branches with sha + protected flag |
| `scm.branches.create`         | Write | Create a new branch from a SHA |
| `scm.files.read`              | Read  | Read a file at an optional ref; returns path + content + encoding |
| `scm.prs.list`                | Read  | List PRs/MRs with state + author + limit filters |
| `scm.prs.get`                 | Read  | Fetch a single PR/MR (title, body, head/base branches, author) |
| `scm.prs.diff`                | Read  | Fetch the unified-diff patch + file/add/delete counters |
| `scm.prs.comment`             | Write | Post a top-level comment on a PR/MR |
| `scm.prs.create`              | Write | Open a PR/MR; supports draft=true |
| `issues.list`                 | Read  | List issues with state + labels + author filters |
| `issues.get`                  | Read  | Fetch a single issue (title, body, state, labels, author) |
| `issues.comment`              | Write | Comment on an issue |
| `issues.create`               | Write | Open a new issue with title + body + labels |
| `issues.update_state`         | Write | Transition an issue to `open` or `closed` |
| `github.workflows_dispatch`   | Write | Trigger a GitHub Actions workflow_dispatch |
| `gitlab.pipeline_trigger`     | Write | Trigger a GitLab CI pipeline against a ref |

Schemas are auto-generated from the typed Args structs and exposed on every
`tools/list` response.

## Configuration

```toml
# ~/.rupu/config.toml or <repo>/.rupu/config.toml
[scm.default]
platform = "github"
owner = "section9labs"
repo = "rupu"

[issues.default]
tracker = "github"
project = "section9labs/rupu"

[scm.github]
base_url = "https://api.github.com"            # overridable for GHES
timeout_ms = 30000
max_concurrency = 8
clone_protocol = "https"                       # https | ssh

[scm.gitlab]
base_url = "https://gitlab.com/api/v4"         # overridable for self-hosted
timeout_ms = 30000
max_concurrency = 6
clone_protocol = "https"
```

## Concurrency, caching, retry

| Platform | Concurrency | Cache TTL | Retry budget |
|----------|:-----------:|:---------:|:------------:|
| github   | 8 permits   | 5 min     | 5 attempts   |
| gitlab   | 6 permits   | 5 min     | 5 attempts   |

Override per-platform via `[scm.<platform>].max_concurrency`.

## Error classification

| HTTP signal                                   | rupu variant      | Recoverable? |
|-----------------------------------------------|-------------------|:------------:|
| 401                                           | `Unauthorized`    | no           |
| 403 + missing-scope header (X-OAuth-Scopes / WWW-Authenticate) | `MissingScope`    | no |
| 403 (other), 429                              | `RateLimited`     | yes          |
| 404                                           | `NotFound`        | yes          |
| 409 / 422 (write conflict keywords)           | `Conflict`        | yes          |
| 422 (validation), 400                         | `BadRequest`      | no           |
| 5xx                                           | `Transient`       | yes          |
| Connection refused / timeout                  | `Network`         | no           |

## Troubleshooting

| Symptom                                              | Likely cause                            | Fix |
|-----------------------------------------------------|-----------------------------------------|-----|
| `MissingScope { scope: "repo" }`                    | PAT was issued without `repo` scope     | `rupu auth logout --provider github && rupu auth login --provider github --mode sso` |
| `RateLimited` after a few calls                     | Hit GitHub's secondary rate limit       | Drop `[scm.github].max_concurrency` to 4 |
| `Unauthorized` after a token rotation               | Keychain still has the old token        | `rupu auth logout --provider github --mode api-key` |
| `Network` from inside a container                   | Container can't reach api.github.com    | Confirm DNS + outbound TCP/443 |
| `tool not in agent's tools: list`                   | Agent forgot to allowlist the tool      | Add `scm.*` (or specific tool name) to frontmatter |
| `gitlab: 403 + insufficient_scope`                  | PAT missing `read_repository`           | Re-issue PAT with full scope set |

## See also

- `docs/scm/github.md` — GitHub-specific walkthrough (PAT, OAuth, GHES)
- `docs/scm/gitlab.md` — GitLab-specific walkthrough (PAT, OAuth, self-hosted)
- `docs/mcp.md` — wiring `rupu mcp serve` into Claude Desktop / Cursor
- `docs/providers.md` — LLM-provider reference (separate auth surface)
