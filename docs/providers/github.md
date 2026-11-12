# GitHub

rupu uses GitHub in two distinct roles:

- **LLM provider (Copilot)** — `provider: copilot` in an agent file routes
  completions through the GitHub Copilot API. See `docs/providers/copilot.md`
  for authentication steps, available models, and configuration knobs.
- **SCM / issue connector** — `rupu auth login --provider github` wires up
  repo, PR, and issue access. Agents call `scm.*` and `issues.*` tools
  regardless of which LLM provider the agent uses.

Both roles log in the same three ways: `--mode api-key` with a personal access
token, `--mode sso`, which is GitHub's device-code flow (rupu prints
`Visit https://github.com/login/device and enter code: …` and polls until you
approve), and `--mode device`, which for GitHub is that same flow under the
mode word every other vendor's headless login uses:

```sh
rupu auth login --provider github --mode device
```

The credential is stored as the account's `sso` entry either way. See
[providers.md](../providers.md#device-login---mode-device) for the polling
rules and `docs/scm.md` for the scopes and `[scm.<account>]` declaration.

## See also

- `docs/providers/copilot.md` — Copilot LLM provider (API key + device-code SSO, model list).
- `docs/scm/github.md` — GitHub repo + issues integration (separate from this LLM-provider doc).
