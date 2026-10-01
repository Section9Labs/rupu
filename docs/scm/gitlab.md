# GitLab

## Auth modes

### API key (PAT)

1. `https://gitlab.com/-/user_settings/personal_access_tokens` → "Add new token"
2. Scopes: `api`, `read_user`, `read_repository`, `write_repository`
3. `rupu auth login --provider gitlab --mode api-key --key glpat-xxx`

### OAuth (browser-callback PKCE)

`rupu auth login --provider gitlab --mode sso` opens gitlab.com's authorize
endpoint in the default browser; rupu listens on `localhost:7171` for the
redirect, completes the PKCE exchange, and stores the token in
`~/.rupu/auth.json`.

On gitlab.com rupu logs in as GitLab's own CLI's public OAuth application —
glab's, the way GitHub SSO uses `gh`'s — so the consent screen names glab. Its
registration fixes the redirect URI (`http://localhost:7171/auth/redirect`) and
the scopes rupu requests (`openid profile read_user write_repository api`); port
7171 must be free while you log in.

GitLab OAuth access tokens expire two hours after issue and the refresh token
rotates on every refresh. Every GitLab connector checks its token before each
request and refreshes it through the credential store when it is within five
minutes of expiry. The refresh happens under `auth.json.lock`, so concurrent rupu
processes don't spend the same refresh token twice, and the rotated token is
persisted. A long-running `rupu cp serve`, `rupu mcp serve` or session keeps
working past the two hours. A refresh that fails while the token still works
(the token endpoint briefly down) is retried after 30 seconds, and the token is
used meanwhile. Once the token no longer works and the refresh fails (the grant
was revoked, say), requests fail as unauthorized with the command that fixes it:
`rupu auth login --account <account> --mode sso`. Processes that are already
running pick up the new login on their next request: a request GitLab refuses
(401) is retried once with the token now in the store.

#### Self-managed GitLab

glab's application exists only on gitlab.com, so a self-managed instance needs
its own:

1. On the instance, create an OAuth application (Admin Area → Applications for
   an instance-wide one, or User settings → Applications) with redirect URI
   `http://localhost:7171/auth/redirect`, scopes `openid profile read_user
   write_repository api`, and **Confidential** unchecked.
2. Put its Application ID next to the instance's API root in
   `~/.rupu/config.toml` (the global config, the only one `auth login` reads):

   ```toml
   [scm.gl-corp]
   kind = "gitlab"
   base_url = "https://gitlab.example.com/api/v4"
   oauth_client_id = "<Application ID>"
   ```

3. `rupu auth login --account gl-corp --mode sso`.

The OAuth endpoints are derived from `base_url` (`<instance>/oauth/authorize`,
`<instance>/oauth/token`, keeping a relative URL root such as
`https://example.com/gitlab`). The login records the application and token
endpoint on the stored credential, so refreshes go back to the instance and
application that issued it, even if the config changes later. Without
`oauth_client_id`, SSO for a self-managed account is refused up front with these
instructions. `oauth_client_id` also works on gitlab.com, to log in as an
application of your own instead of glab's (registered with the same redirect URI
and scopes).

rupu can't clone from a self-managed instance yet: its GitLab clone URLs always
point at gitlab.com (see `TODO.md`), which would receive the account's token, so
`clone_to` (`rupu run` / session repo targets) refuses with an error for such an
account. Clone the repository with `git` directly; API calls (issues, MRs, file
reads, pipelines, event polling) work.

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
- **Self-hosted GitLab**: set `[scm.gitlab].base_url` to the API root,
  including `/api/v4` (e.g. `https://gitlab.example.com/api/v4`) — the API
  connectors and the event poller both append paths to it as-is. The override
  works but is not formally tested in nightly CI; report breakage if you
  depend on this. Clones are refused for now (see "Self-managed GitLab" above).
- **Trigger tokens**: `gitlab.pipeline_trigger` uses the account's token (a PAT
  with `api` scope, or the SSO token), not a separate trigger token.
- **`/changes` endpoint**: the diff endpoint is the legacy `/merge_requests/:iid/changes`;
  GitLab is migrating to `/diffs` for SaaS, but `/changes` is still supported.

## See also

- `docs/scm.md` — canonical reference
