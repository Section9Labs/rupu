# rupu multi-account providers — design

**Date:** 2026-08-28
**Status:** approved (brainstorm), implementation in two arcs
**Scope:** named account identities for every credential-bearing provider — LLM vendors (Anthropic, OpenAI, Gemini, Copilot) and SCM / issue-tracker platforms (GitHub, GitLab, Linear, Jira).

## 1. Problem

rupu supports exactly one credential per provider. The provider *name* is overloaded to mean two different things at once:

1. **Identity** — whose token this is.
2. **Vendor kind** — which client implementation to build.

Because those are the same string, there can only ever be one Anthropic account, one GitHub account, and so on.

Concretely:

- `crates/rupu-auth/src/account_key.rs` keys credentials `<provider>/<mode>`, so there is exactly one `anthropic/api-key` and one `github/sso` slot.
- `crates/rupu-runtime/src/provider_factory.rs` (`build_for_provider_with_config`) matches on the provider **name** to select a client: `"anthropic" => build_anthropic(...)`. A second Anthropic account cannot be named, because any name that is not a built-in falls to the `openai-compatible` arm.
- `crates/rupu-scm/src/registry.rs` stores connectors in `HashMap<Platform, Arc<dyn RepoConnector>>`, where `Platform` is a two-variant enum (`Github | Gitlab`). One connector per platform, cached at discovery.
- `crates/rupu-scm/src/connectors/github/mod.rs` hardcodes `resolver.get("github", None)`.
- `[scm.github]` holds a single `base_url`, so github.com and a GitHub Enterprise host cannot coexist.

Users with a work identity and a personal identity — the common case — cannot express it. SSH host aliases (`git@github-work:...`) solve this for `git` itself but do nothing for GitHub **API** calls, which is what rupu makes.

## 2. Non-goals

- No change to the on-disk credential key format. See §4.
- No migration step, no deprecation cycle, no flag day.
- No multi-tenancy or credential sharing between users.
- No per-account permission or policy model. Accounts are identities only.

## 3. The identity model

Split the two jobs the provider name currently does:

| Concept | Today | After |
| --- | --- | --- |
| **Account name** | doubles as the vendor | the identity. Arbitrary string. Config table name, credential key prefix, registry key. |
| **`kind`** | exists on `ProviderConfig`, accepts only `"openai-compatible"` | the vendor. Selects the client implementation. |

```toml
[providers.anthropic-work]
kind = "anthropic"

[providers.anthropic-personal]
kind = "anthropic"

[scm.gh-work]
kind = "github"

[scm.gh-personal]
kind = "github"

[scm.acme-ghe]
kind = "github"
base_url = "https://git.acme.internal/api/v3"
```

### 3.1 Back-compat rule (load-bearing)

> `kind` defaults to the account name when that name is a known vendor.

An existing user with `anthropic/api-key` stored and no `[providers.*]` config at all keeps working unchanged, forever — as the account that happens to be *named* `anthropic`. Multi-account is purely additive.

This is why the credential key format does **not** change (§4).

## 4. Credential storage

Keys stay `<account>/<mode>`, exactly as `crates/rupu-auth/src/account_key.rs::account_for` writes them today.

`anthropic/api-key` is not migrated to anything. It is simply the account named `anthropic`, in api-key mode.

An earlier proposal to re-key as `<account>/<provider>` was **rejected**: it inverts the existing format and strands every `auth.json` on disk. `account_key.rs` carries a test (`key_format_is_stable_across_providers`) whose comment reads "If this test needs updating, you are about to strand people's credentials." That test must still pass unchanged after this work.

The named-account read/write path already exists and is already correct: `store_named` / `forget_named` / `peek_named` / `get_named` in `crates/rupu-auth/src/resolver.rs` all compose `<name>/<mode>`. It is only quarantined to `openai-compatible` endpoints by its callers.

`CredentialResolver::get` in `crates/rupu-auth/src/resolver.rs` already has the right shape — a single name lookup with a fallback:

```rust
let p = match Self::parse_provider(provider) {
    Ok(p) => p,
    Err(_) => return self.get_named(provider).await,
};
```

## 5. Arc 1 — LLM named accounts

Self-contained and independently shippable. Delivers the multi-SSO story for Anthropic and OpenAI. Selection is already solved: agent frontmatter `provider:` (`crates/rupu-agent/src/spec.rs`, `pub provider: Option<String>`) is already a free-form string.

### 5.1 Extend `kind`

`ProviderConfig.kind` (`crates/rupu-config/src/provider_config.rs`) accepts the built-in vendor names in addition to `"openai-compatible"`: `anthropic`, `openai`, `openai_codex`, `codex`, `gemini`, `google_gemini`, `copilot`, `github_copilot`, `local`.

Validation in `crates/rupu-config/src/config.rs` currently requires `default_model` when `kind == "openai-compatible"`. That requirement stays scoped to `openai-compatible` only — built-in kinds have vendor defaults.

An unknown `kind` is a hard config error naming the valid values.

### 5.2 Dispatch on kind, not name

`build_for_provider_with_config` in `crates/rupu-runtime/src/provider_factory.rs` matches on the **resolved kind** rather than the account name. Resolution:

1. `[providers.<name>].kind` when set.
2. Otherwise the account name itself, when it is a known built-in (§3.1).
3. Otherwise error.

`is_builtin_provider` stays as the vendor-name predicate and becomes the `kind` validator.

### 5.3 Resolver must know the account list

`KeychainResolver::parse_provider` cannot today tell `anthropic-work` (a valid account) from a typo. It needs the declared account list.

`rupu-auth` must NOT depend on `rupu-config` (hexagonal rule 1). Therefore:

```rust
KeychainResolver::with_accounts(accounts: Vec<AccountSpec>)
```

where `AccountSpec { name: String, kind: String }` is defined in `rupu-auth`. The CLI resolves config and passes the list at construction. `new()` stays as "built-ins only", preserving every existing call site's behavior.

### 5.4 SSO refresh routed by kind

`refresh_inner` in `crates/rupu-auth/src/resolver.rs` calls `crate::oauth::providers::provider_oauth(p)` with a `ProviderId` derived from the name. For a named account this must resolve from the account's **kind**.

This is what makes two independent Anthropic SSO tokens work — each refreshes against the same OAuth config under its own account key. Without it, Arc 1 delivers multi-account API keys but not multi-account SSO.

OAuth flows per vendor (verified in `crates/rupu-auth/src/oauth/providers.rs`):

| Vendor | Flow |
| --- | --- |
| Anthropic | `Callback` (browser) |
| OpenAI | `Callback` |
| Gemini | `Callback` |
| Copilot | `Device` |
| GitHub | `Device` |
| Local | none |

### 5.5 CLI

`rupu auth login` gains:

- `--account <name>` — the identity. `--provider` is kept as an alias.
- `--kind <vendor>` — required when the account is not already declared in config; optional (and validated for agreement) when it is.

When `--kind` is supplied for an undeclared account, `auth login` writes `[providers.<name>] kind = "<vendor>"` into the global `config.toml`. This removes the need to hand-edit TOML before authenticating, and it means the account is declared by the time any resolver needs the list (§5.3).

`rupu auth logout` and `rupu auth status` take `--account` likewise.

`auth status` gains a `KIND` column. It already walks `cfg.providers` and appends named entries (`crates/rupu-cli/src/cmd/auth.rs`); it must stop filtering those to `kind == "openai-compatible"` and list every declared account with its kind and per-mode presence.

### 5.6 Env var fallback

Built-in providers have no env-var fallback in the resolver today; only named accounts do, via `RUPU_<UPPER_NAME>_API_KEY` in `get_named`. That generalizes unchanged: every account gets `RUPU_<UPPER_ACCOUNT>_API_KEY`. Non-alphanumeric characters in an account name map to `_`.

## 6. Arc 2 — SCM named accounts

Roughly 3-4x Arc 1's work. Depends on Arc 1's identity model but is otherwise independent.

### 6.1 Registry reshape

`Platform` stays exactly as it is — it is the vendor, and `RepoRef.platform` (`crates/rupu-scm/src/types.rs`) is correctly typed. Only the map key changes:

```rust
pub struct AccountId(String);

struct Account {
    kind: Platform,
    repo: Arc<dyn RepoConnector>,
    issues: Option<Arc<dyn IssueConnector>>,
    events: Option<Arc<dyn EventConnector>>,
    extras: Extras,
}
// Registry: HashMap<AccountId, Account>
```

`[scm.<name>]` becomes account-keyed. `ScmPlatformConfig` gains `kind`. The reserved-`default`-key handling in `platforms_serde` (`crates/rupu-config/src/scm_config.rs`) still applies.

### 6.2 Three accessor shapes

Derived from the actual call sites, not invented:

| Category | Examples | Resolution |
| --- | --- | --- |
| **Targeted** (has a `RepoRef`) | `cmd/issues.rs` list, `rupu-mcp/src/tools/scm_repos.rs` `dispatch_get`, `cmd/autoflow.rs` | `repo_for(&RepoRef)` runs the rule engine. The owner is already in scope at these call sites and is currently discarded. |
| **Account-scoped, no repo** | `list_repos()`, `cp_repos.rs` | Fan out across all accounts of that kind; tag rows by account. |
| **Explicit** | `--account`, MCP `account` arg | Direct lookup, no rules. |

The middle category is important: `scm.repos.list` has no owner to key on, so there is nothing to be ambiguous about. Erroring there would be wrong; a union is the only correct answer, and it is a UX improvement (`rupu repos list` shows work and personal repos in one table, tagged).

### 6.3 Rule engine

```toml
[[scm.rules]]
owner   = "acme/*"
account = "gh-work"

[[scm.rules]]
path    = "~/Code/work/*"
account = "gh-work"
```

A pure function with no I/O and no dependencies:

```rust
fn resolve_account(rules: &[Rule], repo: &RepoRef, cwd: Option<&Path>) -> Resolution
```

Modelled on the existing `resolve_configured_default` helper in `crates/rupu-scm/src/registry.rs`, which was extracted for exactly this reason (so the warn-worthy branch is unit-testable without a tracing harness).

**Precedence:** explicit `--account` → owner rule → path rule → sole account of that kind → error.

The fourth tier is the back-compat clause and is load-bearing: with one GitHub account configured, nothing can be ambiguous, so no rules are needed and nothing errors. Every existing single-account user sees zero behavior change.

Owner rules are what make daemons work: a webhook payload or cron poll knows the owner but has no cwd.

### 6.4 Error on ambiguity

Targeted operations only:

```
no account rule matches other/thing
  configured github accounts: gh-work, gh-personal
  fix: rupu scm bind --owner 'other/*' --account <name>
  or:  pass --account <name>
```

This follows the reasoning already recorded in `ScmDefault`'s docs (`crates/rupu-config/src/scm_config.rs`), which deliberately made a default-repo key inert rather than let an ambiguous command silently target the wrong repo.

### 6.5 Daemons

- **Webhook** — `crates/rupu-cli/src/cmd/webhook.rs` (`maybe_build_github_projects_hydrator`) builds one hydrator at daemon startup and reuses it for every inbound payload. Becomes a per-account cache resolved per payload; the payload carries the owner.
- **Cron** — `EventSourceRef::Repo` carries a `RepoRef` and resolves normally. `EventSourceRef::TrackerProject { tracker, project }` (`crates/rupu-scm/src/types.rs`) carries only a project string — no owner, no path. That variant needs an explicit `account` field on the trigger config. It is the one place multi-account cannot be inferred.

### 6.6 `rupu scm bind`

Sugar that appends `[[scm.rules]]` to config:

```bash
rupu scm bind --owner 'acme/*' --account gh-work
rupu scm bind --path '~/Code/work/*' --account gh-work
```

Optional for Arc 2 v1; hand-editing `[[scm.rules]]` is an acceptable fallback.

## 7. Acceptance walkthrough

This is the user experience the implementation must produce. It is the acceptance criterion for both arcs.

```bash
# Arc 1
rupu auth login --account anthropic-work     --kind anthropic --mode sso
rupu auth login --account anthropic-personal --kind anthropic --mode sso
rupu auth login --account openai-work        --kind openai    --mode sso
rupu auth login --account openai-personal    --kind openai    --mode api-key

# Arc 2
rupu auth login --account gh-work     --kind github --mode sso
rupu auth login --account gh-personal --kind github --mode sso

rupu scm bind --owner 'acme/*'        --account gh-work
rupu scm bind --owner 'MrBrutti/*'    --account gh-personal
rupu scm bind --path  '~/Code/work/*' --account gh-work
```

Real output, captured from the built binary against a scratch `RUPU_HOME`
with exactly the six accounts above declared and stored (`rupu auth
status`, table renderer, unedited). Note two things this shorthand
sample got wrong: **the eight built-in vendor names are always listed
first, unconditionally** — this is a back-compat requirement, so a
single-account user with no `[providers.*]` config still sees the same
table they see today — with declared accounts appended after them in
account-name order; and a present SSO credential renders with a leading
`✓`, not bare `expires in Nd`. (The day counts below are one day lower
than the login commands' "27d / 30d / 8d" because the sub-second gap
between storing and reading truncates the last partial day — an
artifact of capture timing, not a real precision guarantee.)

```
$ rupu auth status
 ACCOUNT              KIND        API KEY   SSO
──────────────────── ─────────── ───────── ──────────────────
 anthropic            anthropic   —         —
 openai               openai      —         —
 gemini               gemini      —         —
 copilot              copilot     —         —
 github               -           —         —
 gitlab               -           —         —
 linear               -           —         —
 jira                 -           —         —
 anthropic-personal   anthropic   —         ✓ expires in 29d
 anthropic-work       anthropic   —         ✓ expires in 26d
 gh-personal          github      —         ✓ no expiry
 gh-work              github      —         ✓ no expiry
 openai-personal      openai      ✓         —
 openai-work          openai      —         ✓ expires in 7d
```

(The four built-in SCM/issue-tracker vendors — `github`, `gitlab`,
`linear`, `jira` — show `KIND -` in their built-in row: they have no
`[providers.<name>]` declaration of their own, and unlike the four LLM
vendors, `rupu-runtime`'s kind resolver deliberately doesn't fall back
to the bare name for them — see `rupu_runtime::provider_factory::
is_builtin_provider`'s doc comment. This is existing, correct behavior,
not something this arc changed.)

```yaml
---
name: work-reviewer
provider: anthropic-work
model: claude-opus-4-6
---
```

```bash
rupu issues list --repo acme/api        # -> gh-work, via owner rule
rupu issues list --repo MrBrutti/dots   # -> gh-personal
rupu repos list                         # -> BOTH, one table, tagged
rupu issues list --repo other/thing --account gh-work   # explicit override
```

Resulting `~/.rupu/auth.json` (chmod 600):

```json
{
  "anthropic-work/sso":      "{...}",
  "anthropic-personal/sso":  "{...}",
  "openai-work/sso":         "{...}",
  "openai-personal/api-key": "{...}",
  "gh-work/sso":             "{...}",
  "gh-personal/sso":         "{...}"
}
```

## 8. Testing

- `account_key.rs::key_format_is_stable_across_providers` must pass **unchanged**.
- A user with only `anthropic/api-key` and empty config resolves Anthropic exactly as before (regression test for §3.1).
- Two accounts of the same kind store, read, and refresh independently.
- `resolve_account` is unit-tested as a pure function across the full precedence table, including the sole-account and no-match branches.
- Single-account SCM setups never hit the ambiguity error.
- Fan-out listing returns the union, tagged by account.
- `parse_provider` rejects an undeclared name while accepting a declared one.

## 9. Naming convention

Account names are free-form and unvalidated. Because the account name is what appears in every agent's frontmatter, an inconsistent scheme becomes annoying. Documented convention: `<vendor>-<context>` (`anthropic-work`, `gh-personal`). Not enforced.
