# GitHub

rupu uses GitHub in two distinct roles:

- **LLM provider (Copilot)** — `provider: copilot` in an agent file routes
  completions through the GitHub Copilot API. See `docs/providers/copilot.md`
  for authentication steps, available models, and configuration knobs.
- **SCM / issue connector** — `rupu auth login --provider github` wires up
  repo, PR, and issue access. Agents call `scm.*` and `issues.*` tools
  regardless of which LLM provider the agent uses.

## See also

- `docs/providers/copilot.md` — Copilot LLM provider (API key + device-code SSO, model list).
- `docs/scm/github.md` — GitHub repo + issues integration (separate from this LLM-provider doc).
