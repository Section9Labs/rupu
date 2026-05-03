# rupu — agentic code-development CLI

## Read first
- Slice A spec: `docs/superpowers/specs/2026-05-01-rupu-slice-a-design.md`
- Slice B-1 spec: `docs/superpowers/specs/2026-05-02-rupu-slice-b1-multi-provider-design.md`
- Plan 1 (foundation & API-key wiring, in progress): `docs/superpowers/plans/2026-05-02-rupu-slice-b1-plan-1-foundation-and-api-key.md`
- Plan 2 (SSO flows): `docs/superpowers/plans/2026-05-02-rupu-slice-b1-plan-2-sso-and-resolver.md`
- Plan 3 (model resolution & polish): `docs/superpowers/plans/2026-05-02-rupu-slice-b1-plan-3-models-and-polish.md`

## Architecture rules (enforced)
1. **Hexagonal separation.** `rupu-providers`, `rupu-tools`, `rupu-auth` define traits (ports). The agent runtime in `rupu-agent` only knows traits.
2. **`rupu-cli` is thin.** Subcommands are arg parsing + delegation. No business logic in the CLI crate.
3. **Workspace deps only.** Versions pinned in root `Cargo.toml`; never in crate `Cargo.toml` files.
4. `#![deny(clippy::all)]` workspace-wide via `[workspace.lints]`. `unsafe_code` forbidden.

### Crates

- **`rupu-agent`** — agent file format (`.md` + YAML frontmatter), agent loop, and permission resolver. Lifts spec/loader/permission/runner/tool_registry into one integration crate. Mock-provider tests use `MockProvider` + `BypassDecider` exposed from `runner`.
- **`rupu-orchestrator`** — workflow YAML parser + minijinja rendering + linear runner with pluggable `StepFactory`. Action-protocol allowlist validation lives here.
- **`rupu-cli`** — the `rupu` binary. Thin clap dispatcher to the libraries. Seven subcommands: `run` / `agent` / `workflow` / `transcript` / `config` / `auth`.

**Run-time samples:** live at `<repo>/.rupu/agents/` and `<repo>/.rupu/workflows/`. Running `rupu` from inside the rupu checkout exercises the same project-discovery code path end-users use in their own repos.

## Code standards
- Rust 2021, MSRV pinned in `rust-toolchain.toml`.
- Errors: `thiserror` for libraries; `anyhow` for the CLI binary (Plan 2).
- Async: `tokio`.
- Logging: `tracing` + `tracing-subscriber`.

## Heritage
- **Okesu** (`/Users/matt/Code/Oracle/Okesu`) — Go security-ops sibling. Same architectural shape (agent files = `.md` + YAML, JSONL transcripts, action protocol).
- **phi-cell** (`/Users/matt/Code/phi-cell`) — Rust workspace; `crates/phi-providers` is lifted near-verbatim into `crates/rupu-providers`. Lift origin: `Section9Labs/phi-cell` commit `3c7394cb1f5a87088954a1ff64fce86303066f55`.
