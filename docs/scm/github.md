# GitHub

## Auth modes

### API key (PAT)

1. `https://github.com/settings/tokens` → "Generate new token (classic)"
2. Scopes: `repo`, `workflow`, `gist`, `read:org`, `read:user`
3. `rupu auth login --provider github --mode api-key --key ghp_xxx`

### OAuth (device-code SSO)

`rupu auth login --provider github --mode sso` prints a verification URL +
user-code, opens https://github.com/login/device, prompts for the code, stores
the access token in keychain.

## Sample agent

```yaml
---
name: review-pr
provider: anthropic
model: claude-sonnet-4-6
tools: [scm.prs.get, scm.prs.diff, scm.prs.comment]
permissionMode: ask
---
You are a code reviewer. Read the PR via scm.prs.diff and post a single
summary review with scm.prs.comment.
```

Run: `rupu run review-pr github:section9labs/rupu#42`

## Known quirks

- **Fine-grained PATs**: less surface than classic PATs (no `gist`, less `read:org`).
  rupu uses classic PATs by default; document this in agent configs that need
  fine-grained scoping.
- **GraphQL**: rupu uses REST only in v0; some queries (e.g. cross-org search)
  aren't reachable. Filed as out-of-scope.
- **GHES**: set `[scm.github].base_url = "https://ghes.example.com/api/v3"`.
  No code changes required.
- **Workflow dispatch**: requires `workflow` scope on the PAT *and* the workflow
  file must contain `on: workflow_dispatch:`. rupu surfaces 422 as
  `BadRequest { message: "workflow not configured for dispatch" }`.

## See also

- `docs/scm.md` — canonical reference
- `docs/providers/github.md` — Copilot LLM provider (separate keychain entry)
