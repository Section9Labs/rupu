# rupu — agentic code-development CLI

## Read first
- Slice A spec: `docs/superpowers/specs/2026-05-01-rupu-slice-a-design.md`
- Slice B-1 spec: `docs/superpowers/specs/2026-05-02-rupu-slice-b1-multi-provider-design.md`
- Slice B-2 spec: `docs/superpowers/specs/2026-05-03-rupu-slice-b2-scm-design.md`
- Plan 1 (foundation + GitHub connector, complete): `docs/superpowers/plans/2026-05-03-rupu-slice-b2-plan-1-foundation-and-github.md`
- Plan 2 (GitLab + MCP server, complete): `docs/superpowers/plans/2026-05-03-rupu-slice-b2-plan-2-gitlab-and-mcp.md`
- Plan 3 (CLI run-target + docs + nightly, complete): `docs/superpowers/plans/2026-05-03-rupu-slice-b2-plan-3-cli-and-docs.md`

## Architecture rules (enforced)
1. **Hexagonal separation.** `rupu-providers`, `rupu-tools`, `rupu-auth` define traits (ports). The agent runtime in `rupu-agent` only knows traits.
2. **`rupu-cli` is thin.** Subcommands are arg parsing + delegation. No business logic in the CLI crate.
3. **Workspace deps only.** Versions pinned in root `Cargo.toml`; never in crate `Cargo.toml` files.
4. `#![deny(clippy::all)]` workspace-wide via `[workspace.lints]`. `unsafe_code` forbidden.

### Crates

- **`rupu-agent`** — agent file format (`.md` + YAML frontmatter), agent loop, and permission resolver. Lifts spec/loader/permission/runner/tool_registry into one integration crate. Mock-provider tests use `MockProvider` + `BypassDecider` exposed from `runner`.
- **`rupu-cli`** — the `rupu` binary. Thin clap dispatcher to the libraries. Nine subcommands: `run` / `agent` / `workflow` / `transcript` / `config` / `auth` / `models` / `repos` / `mcp`.
- **`rupu-mcp`** — embedded MCP server. Two transports (in-process for the agent runtime, stdio for `rupu mcp serve`); single tool catalog backed by `rupu-scm`'s Registry. Permission gating mirrors the six-builtin model: per-tool allowlist + per-mode (`ask` / `bypass` / `readonly`).
- **`rupu-orchestrator`** — workflow YAML parser + minijinja rendering + linear runner with pluggable `StepFactory`. Action-protocol allowlist validation lives here.
- **`rupu-scm`** — SCM/issue-tracker connectors. `RepoConnector` + `IssueConnector` traits per spec §4c; per-platform impls under `connectors/<platform>/`. Plan 1 ships GitHub; Plan 2 adds GitLab + the embedded MCP server.

**Run-time samples:** live at `<repo>/.rupu/agents/` and `<repo>/.rupu/workflows/`. Running `rupu` from inside the rupu checkout exercises the same project-discovery code path end-users use in their own repos.

## Code standards
- Rust 2021, MSRV pinned in `rust-toolchain.toml`.
- Errors: `thiserror` for libraries; `anyhow` for the CLI binary (Plan 2).
- Async: `tokio`.
- Logging: `tracing` + `tracing-subscriber`.

## Heritage
- **Okesu** (`/Users/matt/Code/Oracle/Okesu`) — Go security-ops sibling. Same architectural shape (agent files = `.md` + YAML, JSONL transcripts, action protocol).
- **phi-cell** (`/Users/matt/Code/phi-cell`) — Rust workspace; `crates/phi-providers` is lifted near-verbatim into `crates/rupu-providers`. Lift origin: `Section9Labs/phi-cell` commit `3c7394cb1f5a87088954a1ff64fce86303066f55`.
