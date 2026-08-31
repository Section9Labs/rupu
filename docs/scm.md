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
| Issues (read, comment, create, transition) | ✅ |   ✅   |   ✅   |   ✅  |
| Issue comment-thread read (`issues.comments`) | ✅ |   —    |   —    |   —  |
| Workflow / pipeline trigger         |   ✅   |   ✅   |   —    |   —  |
| `clone_to` (local checkout)         |   ✅   |   ✅   |   —    |   —  |
| File read by ref                    |   ✅   |   ✅   |   —    |   —  |
| API surface                         |  REST  |  REST  | GraphQL| REST |

Linear and Jira implement every `IssueConnector` method except
`list_comments` (`list_issues`, `get_issue`, `comment_issue`, `create_issue`,
`update_issue_state`), so the `issues.*` MCP tools (`issues.list`,
`issues.get`, `issues.comment`, `issues.create`, `issues.update_state`) work
against them today, in addition to their native trigger sources (webhook +
polling). `issues.comments` (comment-thread read) is GitHub-only; the default
`IssueConnector::list_comments` impl returns "not supported" for Linear and
Jira. They are not full repo / PR backends — no `scm.*` repo, branch, PR/MR,
or clone support.

## Auth

> New project? Run `rupu init --with-samples` to seed `.rupu/agents/scm-pr-review.md` and the rest of the curated templates.

`rupu auth login --provider <github|gitlab|linear|jira> --mode <api-key|sso>` stores tokens
in the OS keychain. Same flow as Slice B-1's LLM-provider auth; `rupu auth status`
picks up SCM rows automatically. `--provider` is an alias for `--account` — a
work identity and a personal identity on the same platform can coexist as two
named accounts (`--account gh-work --kind github`, `--account gh-personal
--kind github`); see `docs/providers.md`'s "Accounts vs. vendor kind" for the
full mechanism.

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

All 18 tools in the unified surface. Each accepts an optional `platform?` (or
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
| `issues.comments`             | Read  | Read an issue's comment thread, oldest-first (GitHub only) |
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

### Field reference

- **`base_url`** (`Option<String>`): API root — override for GHES / self-hosted GitLab. Note the *clone* paths still use the public host; self-hosted clone URLs are tracked separately in `TODO.md`.
- **`timeout_ms`** (`Option<u64>`): total per-request deadline for this platform's HTTP calls. Default: `30000`. `0` is treated as unset.
- **`max_concurrency`** (`Option<usize>`): per-platform semaphore size. Defaults: github 8, gitlab 6.
- **`clone_protocol`** (`"https" | "ssh"`): how `clone_to` reaches the remote. Default `https` (token embedded in the URL). `ssh` produces `git@<host>:<owner>/<repo>.git` and drops the token entirely — authentication is your SSH agent and `~/.ssh/config`. SSH clones shell out to the system `git` so host aliases, `IdentityFile`, `ProxyJump`, and agent forwarding apply; `git` must be on `PATH`. An unrecognized value logs a warning and falls back to `https`.
- **`kind`** (`Option<String>`, `"github"` | `"gitlab"`): the vendor a *named* account talks to — see "Multi-account routing" below. `None` means the table name itself is the vendor (`[scm.github]`, `[scm.gitlab]`), which is why every example above needs no `kind` at all.

## Multi-account routing

A single GitHub account (or a single GitLab account) needs none of this: `[scm.github]` / `[scm.gitlab]` — or no `[scm.*]` config at all, just a stored credential — resolves exactly as shown above, unchanged. This section is for holding **two** accounts of the same platform: a work identity and a personal identity, or github.com alongside a GitHub Enterprise host.

### Accounts

`[scm.<name>]` is account-keyed, not platform-keyed. The table name is the identity (freeform — whatever you pass to `--account`); `kind` selects the vendor client:

```toml
[scm.gh-work]
kind = "github"

[scm.gh-personal]
kind = "github"

[scm.acme-ghe]
kind = "github"
base_url = "https://git.acme.internal/api/v3"    # GitHub Enterprise
```

A bare vendor name (`[scm.github]`, no `kind`) still works with zero change — the account name *is* the vendor, exactly as in the single-account examples above. This is the same identity-vs-vendor split Slice B-1 established for LLM providers; see `docs/providers.md`'s "Accounts vs. vendor kind" for the general mechanism. `rupu auth login --account <name> --kind github|gitlab --mode <api-key|sso>` declares the account (writing `[scm.<name>] kind = "<kind>"` into `~/.rupu/config.toml` the first time that name is used) and stores its credential in the same step — there is no separate "declare, then log in" step.

Run `rupu scm accounts` to see every configured account, its kind, base_url, and which rules (below) point at it — the command an ambiguity error (below) tells you to run.

### Rules

With two accounts of the same platform, rupu needs to know which one serves a given repo. `[[scm.rules]]` supplies that:

```toml
[[scm.rules]]
owner = "acme/*"
account = "gh-work"

[[scm.rules]]
path = "~/Code/work/*"
account = "gh-work"
```

Exactly one of `owner` (matched against the repo owner — `acme/*` or a bare `acme`) or `path` (matched against the caller's cwd — `~/Code/work/*`, `~` expands to `$HOME`) is set per rule; a rule with both or neither fails config validation. `rupu scm bind` (below) is the CLI shortcut for appending one.

**Precedence**, highest first:

1. **Explicit** — `--account <name>` on the command line, or an MCP `account` argument.
2. **Owner rule** — the first `[[scm.rules]]` entry whose `owner` glob matches. This is the tier daemons rely on: a webhook payload or cron poll knows the repo owner but has no cwd.
3. **Path rule** — the first entry whose `path` glob matches the current directory.
4. **Sole account** — exactly one account of the repo's platform is configured, so nothing is ambiguous and no rule is needed. This is the back-compat guarantee: a single-account setup never sees any of this.
5. **Error** — two or more accounts, no explicit selection, nothing matched.

The first matching owner or path rule wins its tier — but if the account that rule names has no live connector **anywhere** (not merely for a different platform, see below), resolution stops right there with an error rather than falling through to a later tier. Before this existed, a rule for `acme/*` naming `gh-work` whose credential was missing/revoked/unrefreshable, with exactly one other account configured, silently resolved through the sole-account tier to that *other* account — a cross-identity misroute with no error at all. See "The unavailable-account error" below.

This is distinct from a rule whose account IS live, just under a different platform than the current call needs — e.g. a path rule naming a GitHub account, evaluated while resolving a GitLab repo from the same directory. `[[scm.rules]]` entries have no platform of their own, so this is a legitimate, supported shape (per-directory routing for two different vendors), and it behaves exactly as it always has: the rule is skipped and the next rule, or a later tier, gets a chance.

A rule naming an account that has no `[scm.<name>]` table logs a warning at config load (not an error — the account may be credential-only, e.g. a bare vendor name that needs no table), but a rule with both `owner` and `path`, or neither, is a hard config error naming the offending entry.

Two commands have no repo to key on at all — `rupu repos list` and the `scm.repos.list` MCP tool — so there is nothing to disambiguate: they fan out across every configured account of the platform and return the union in one table, tagged by account.

### The ambiguity error

A *targeted* command (one naming a specific repo — `rupu issues list --repo`, `scm.repos.get`, …) against a repo no rule matches, with two or more candidate accounts, errors rather than silently guessing:

```
no account rule matches other/thing
  configured github accounts: gh-personal, gh-work
  fix: rupu scm bind --owner 'other/*' --account <name>
  or:  pass --account <name>
```

Fix it either by adding a rule (`rupu scm bind --owner 'other/*' --account gh-work`) or by passing `--account` on that one invocation. `rupu scm accounts` shows the accounts named in the error and what, if anything, already routes to them.

### The unavailable-account error

A rule that matches — its `owner` or `path` glob fires — but names an account with no live connector under any platform errors instead of silently falling through to a different account:

```
the rule for `acme/*` names `gh-work`, which has no live connector for github
  fix: rupu auth login --account gh-work
```

This is a different failure from the ambiguity error above: there, no rule fired at all. Here, one did, and named a specific account — falling through to whatever else happened to be configured would silently target a different identity than the rule chose, exactly the failure mode this whole rule engine exists to prevent. The fix line deliberately omits `--kind`: if `gh-work` is already declared (a stranded `[scm.gh-work]` table from an earlier login, say), `rupu auth login --account gh-work` reuses the declared kind on its own; if it isn't declared at all, `auth login` asks for `--kind` rather than the error guessing one and risking a rewrite of an unrelated, already-working account under the same name.

A rule naming a **live** account that just happens to be of a different platform (see "Precedence" above) does not trigger this error — it isn't a misconfiguration, so it isn't reported as one.

### `rupu scm bind`

Sugar for appending `[[scm.rules]]` — writes via the same `toml_edit`-based atomic config writer `rupu auth login` uses, so comments and formatting in `config.toml` survive and a config that doesn't already parse as valid TOML is left untouched:

```bash
rupu scm bind --owner 'acme/*'        --account gh-work
rupu scm bind --owner 'MrBrutti/*'    --account gh-personal
rupu scm bind --path  '~/Code/work/*' --account gh-work
```

`--owner` and `--path` are mutually exclusive; exactly one is required alongside `--account`. Hand-editing `[[scm.rules]]` works identically — `scm bind` is a convenience, not the only way in.

### The `--account` flag

`rupu issues list` / `rupu issues show`, and the `scm.*`/`issues.*` MCP tools, accept `--account <name>` (CLI) / `account` (MCP tool argument) to bypass the rule engine entirely for that one call — precedence tier 1 above. Useful for a one-off against an account the rules don't cover yet, without editing config first:

```bash
rupu issues list --repo github:other/thing --account gh-work
```

An unconfigured `--account` name errors immediately (`no such account: ...`, listing what *is* configured) rather than silently falling back to some other account.

Run-target commands (`rupu run <target>`, `rupu workflow run <target>`, `rupu issues run` — equivalent to `rupu workflow run <name> <issue-ref>`) have **no** `--account` flag; they resolve strictly by rule (owner → path → sole account). Against an ambiguous repo, the `NoRuleMatched` error's `or: pass --account <name>` line does not apply to them — add a `[[scm.rules]]` entry (`rupu scm bind`) instead.

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
