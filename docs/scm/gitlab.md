# GitLab

## Auth modes

### API key (PAT)

1. `https://gitlab.com/-/user_settings/personal_access_tokens` → "Add new token"
2. Scopes: `api`, `read_user`, `read_repository`, `write_repository`
3. `rupu auth login --provider gitlab --mode api-key --key glpat-xxx`

### OAuth (browser-callback PKCE)

`rupu auth login --provider gitlab --mode sso` opens gitlab.com's authorize
endpoint in the default browser; rupu listens on a fixed loopback port for the
redirect, completes the PKCE exchange, stores the access token in keychain.

> **Note**: gitlab.com OAuth currently uses a placeholder client_id pending
> registration of a rupu-specific OAuth app (TODO.md item). Use the API-key path
> for now; the SSO flow's UX will improve once the app is registered.

## Sample agent

```yaml
---
name: review-mr
provider: anthropic
model: claude-sonnet-4-6
tools: [scm.prs.get, scm.prs.diff, scm.prs.comment]
permissionMode: ask
---
You are a code reviewer. Read the MR via scm.prs.diff and post a single
summary review with scm.prs.comment.
```

Run: `rupu run review-mr gitlab:group/project!7`

## Known quirks

- **MR vs PR vocabulary**: rupu translates internally — agents always see
  `scm.prs.*`. The `target` arg uses `!N` (GitLab convention) instead of `#N`.
- **Nested groups**: `group/sub/project` parses with `owner = "group/sub"`,
  `repo = "project"`. URL-encoded as `group%2Fsub%2Fproject` in API calls.
- **Self-hosted GitLab**: `[scm.gitlab].base_url` override works but is not
  formally tested in nightly CI; report breakage if you depend on this.
- **Trigger tokens**: `gitlab.pipeline_trigger` uses your PAT (with `api` scope),
  not a separate trigger token.
- **`/changes` endpoint**: the diff endpoint is the legacy `/merge_requests/:iid/changes`;
  GitLab is migrating to `/diffs` for SaaS, but `/changes` is still supported.

## See also

- `docs/scm.md` — canonical reference
