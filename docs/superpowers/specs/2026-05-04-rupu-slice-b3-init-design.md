# rupu Slice B-3: `rupu init` — Design

**Status:** Approved
**Date:** 2026-05-04
**Slice:** B-3 (third of three Slice B sub-projects)
**Companion docs:** [Slice A design](./2026-05-01-rupu-slice-a-design.md), [Slice B-1 design](./2026-05-02-rupu-slice-b1-multi-provider-design.md), [Slice B-2 design](./2026-05-03-rupu-slice-b2-scm-design.md)

---

## 1. Goal

A single-command project bootstrap: `rupu init [PATH]` creates rupu's standard layout in a target directory, with optional curated sample agents and workflows. Replaces today's "copy files from the rupu repo" instruction with a self-contained CLI step.

This is the third and final Slice B sub-project. After it merges, Slice B is complete and the focus moves to Slice C (TUI).

## 2. Why

Today, the rupu binary works fine the moment a user authenticates and creates an `.rupu/` directory by hand. Every README and walkthrough has to point at the rupu repo's own `.rupu/` directory and tell the user to copy from it. That's awkward, brittle, and not the experience matt wants for v0 release readiness.

`rupu init --with-samples` ends that awkwardness:
- New users get a working set of templates (review-diff, add-tests, fix-bug, scaffold, summarize-diff, scm-pr-review) plus one workflow (investigate-then-fix) the moment they install the binary.
- The samples are explicitly curated — not "everything in the rupu repo's `.rupu/`" — so test fixtures don't leak into user projects.
- An offline-capable `include_str!` source means `cargo install` users don't need network on first run.

## 3. Architecture

One new CLI subcommand, embedded templates compiled in at build time, no new crate.

| File / dep | Status | Responsibility |
|---|---|---|
| `crates/rupu-cli/templates/` | NEW | Source-of-truth for embedded agent + workflow templates. One file per template. |
| `crates/rupu-cli/src/templates.rs` | NEW | Manifest of `(relative_path, content)` pairs via `include_str!`. Adding a template is a one-line change in this file plus dropping the source under `templates/`. |
| `crates/rupu-cli/src/cmd/init.rs` | NEW | The subcommand handler. ~150 lines of pure logic: walk the manifest, decide per-file action, write, report. |
| `crates/rupu-cli/src/cmd/mod.rs` | MODIFY | `pub mod init;` |
| `crates/rupu-cli/src/lib.rs` | MODIFY | New `Init(InitArgs)` variant on `Cmd`; new dispatcher arm. |
| `crates/rupu-cli/Cargo.toml` | UNCHANGED | No new deps; `std::fs` + the existing `which` workspace dep for git detection. |

Architectural rules preserved:
- `rupu-cli` stays thin (init.rs is arg parsing + small file-writing logic + a manifest walk; no business logic shared with other crates).
- `#![deny(clippy::all)]` workspace-wide.
- `unsafe_code` forbidden.

## 4. CLI surface

```
rupu init [PATH]
  --with-samples            Include the curated agent + workflow set.
  --force                   Overwrite existing files. Default is merge (skip-existing).
  --git                     Run `git init` afterwards if PATH is not already in a git repo.
```

`PATH` defaults to `.` (current working directory).

Exit codes:
- **0** — success.
- **1** — IO/permission error during file write.
- **2** — validation error (e.g. `PATH` exists but is a file, not a directory).

## 5. Skeleton (created by every invocation)

Every `rupu init` invocation, with or without `--with-samples`, creates:

```
<PATH>/
  .rupu/
    agents/                              # empty unless --with-samples
    workflows/                           # empty unless --with-samples
    config.toml                          # commented skeleton
  .gitignore                             # appends `.rupu/transcripts/` if missing; created if absent
```

`config.toml` skeleton content:

```toml
# rupu project config — see https://github.com/Section9Labs/rupu/blob/main/docs/providers.md

# default_model = "claude-sonnet-4-6"

# [scm.default]
# platform = "github"
# owner = "<your-org>"
# repo = "<this-repo>"

# [issues.default]
# tracker = "github"
# project = "<your-org>/<this-repo>"
```

`.gitignore` handling:
- File missing → create it with a single line: `.rupu/transcripts/`.
- File present and missing the line → append the line (with a leading newline if the existing file doesn't end with one).
- File present and contains the line → no change.

The `.gitignore` is the ONLY file rupu touches outside `.rupu/`. The append is idempotent.

## 6. Sample content (`--with-samples`)

Curated set, embedded via `include_str!`. The manifest in `crates/rupu-cli/src/templates.rs` is the single source-of-truth for what ships:

```
templates/agents/
  review-diff.md
  add-tests.md
  fix-bug.md
  scaffold.md
  summarize-diff.md
  scm-pr-review.md
templates/workflows/
  investigate-then-fix.yaml
```

These are copies of the equivalent files in the rupu repo's own `.rupu/agents/` and `.rupu/workflows/`. The four `sample-<provider>.md` files in the rupu repo are explicitly EXCLUDED — they're test fixtures for slice B-1 / B-2 development, not user-facing templates. The same goes for any future test-fixture agents: they live in `.rupu/` for dogfooding but are not in the `crates/rupu-cli/templates/` manifest.

Maintenance: the curated list will grow over time. Adding a new template is:
1. Drop the file under `crates/rupu-cli/templates/agents/<name>.md` (or `workflows/<name>.yaml`).
2. Add one line to the manifest array in `templates.rs`.

The CI gate ensures the manifest stays in sync with the directory contents — see §10.

## 7. Merge / overwrite semantics

The "merge by default; `--force` overwrites" answer to brainstorm Q3.

For each file in (skeleton ∪ samples):
- File doesn't exist → write template content. Action: `CREATED <path>`.
- File exists, no `--force` → skip. Action: `SKIPPED <path> (exists)`.
- File exists, `--force` → overwrite. Action: `OVERWROTE <path>`.

`.gitignore` is special: see §5. `--force` does NOT cause rupu to add a second `.rupu/transcripts/` line, and does NOT touch any other lines.

`--force` only affects files in the manifest. It never touches user-written agents/workflows that aren't in the curated set, never `.rupu/transcripts/`, never `.rupu/cache/`. Out-of-manifest paths are out of scope.

Final stdout summary: `init: created N, skipped M, overwrote K`.

## 8. `--git` flag

After file writes complete, if `--git` was passed:
1. Check whether `PATH` is inside a git repo: shell out to `git rev-parse --is-inside-work-tree` (working directory = `PATH`). Suppress stderr.
2. If outside a repo: run `git init` in `PATH` (sync, default branch = whatever git is configured for).
3. If inside a repo: no-op.
4. If `git` is not on PATH at all: print a warning to stderr (`init: --git requested but git not found on PATH; skipping`); exit code stays 0.

Failure to `git init` (e.g. permission error) is reported as a stderr warning, not a hard failure — the file writes already succeeded and that's the more important step.

`--git` is opt-in and idempotent. It's safe to pass on re-runs.

## 9. Discovery (no change)

The output of `rupu init` is the SAME directory layout that `rupu run` / `rupu workflow run` already discover via the existing project-discovery code path (Slice A). No changes to discovery semantics. After `rupu init --with-samples`, `rupu run review-diff` immediately works (assuming an LLM provider is configured via `rupu auth login`).

## 10. Testing strategy

### 10a. Unit / integration tests in `crates/rupu-cli/tests/`

- `init_create_skeleton.rs` — empty TempDir → `rupu init` → assert `.rupu/{agents,workflows}/` exist + `config.toml` content matches the skeleton + `.gitignore` has the transcripts line.
- `init_with_samples.rs` — empty TempDir → `rupu init --with-samples` → assert all six agents + one workflow exist with the embedded content. Also assert content matches the source files in `.rupu/` (proves the manifest is the same byte-for-byte).
- `init_merge_behavior.rs` — pre-create one agent file → run `rupu init --with-samples` → assert that file was SKIPPED, others were CREATED.
- `init_force.rs` — pre-create one agent with stub content → run `rupu init --with-samples --force` → assert content was overwritten with the template.
- `init_gitignore.rs` — three sub-cases: missing → created; present without entry → entry appended; present with entry → unchanged.
- `init_git_flag.rs` — empty dir + `--git` → `.git/` exists. Same dir twice → second run is no-op (no double-init failure).

### 10b. Manifest sync test

`init_manifest_in_sync.rs` walks `crates/rupu-cli/templates/` and asserts every file in there appears in the manifest. Prevents the "file added on disk but forgotten in code" failure mode.

A second assertion: every entry in the manifest appears as an actual file under `templates/`. Catches typos.

### 10c. Smoke (binary, end-to-end)

`init_smoke.rs` — spawn the release-built `rupu` binary against a TempDir, parse its stdout (CREATED/SKIPPED/OVERWROTE lines), assert the file tree matches expectations. Single test; the unit tests cover the matrix.

### 10d. Honors "no mock features" rule

Every code path either writes the real template content or returns an explicit error. No silent fallbacks. The manifest-sync test ensures discovery never silently misses a template.

## 11. Documentation

- **README.md**: update the "Quick start" section to recommend `rupu init --with-samples --git` for new projects. Replace the "copy from rupu repo" instruction.
- **CHANGELOG.md**: new entry for v0.3.0 (this slice's release vehicle):
  - Adds `rupu init [--with-samples] [--force] [--git]`.
  - Curated template set (6 agents + 1 workflow) embedded via `include_str!`.
  - `.gitignore` and config skeleton bootstrapping.
- **`docs/scm.md`**: cross-reference `rupu init --with-samples` from the quick-start at the top of the page.
- **CLAUDE.md**: bump rupu-cli's subcommand count from 9 → 10; add `init` to the list.
- **In-code docs**: `cmd/init.rs` carries a module docstring explaining the manifest-driven design.

## 12. Out of scope

Deferred to follow-up slices:

- **`rupu init --interactive`** (prompt for default provider/model and write `config.toml` with values filled). Adds clap-prompt complexity; not needed for v0.
- **Template versioning / migration.** Today users re-run `rupu init --with-samples --force` to refresh; a future `rupu update-samples` could be smarter (3-way merge, pin per-template versions).
- **Project-name detection.** No template substitution today (`<your-org>` etc. are literal placeholders the user edits). A future `--project-name <name>` flag could substitute on write.
- **Non-rupu scaffolding.** `rupu init` does NOT create README.md, LICENSE, or any other generic project files. The `--git` flag is the only "outside `.rupu/`" affordance.
- **Sample updates over time.** Maintained manually via PRs to `templates/`. No auto-pull from a remote source.

## 13. Risks

- **Template / source drift.** The `.rupu/` files in the rupu repo are dogfooded by every `rupu run` from inside the checkout, but the `crates/rupu-cli/templates/` copies are the user-facing source. If the two get out of sync, dogfooders see one thing and users see another. **Mitigation**: a CI check (`cargo test -p rupu-cli --test init_with_samples`) compares the byte content of `templates/agents/review-diff.md` against `.rupu/agents/review-diff.md` and fails on mismatch. Regenerating one from the other is a `cp` away.
- **Force-overwrite of customized samples.** If a user copies a template, customizes it, and later runs `rupu init --with-samples --force` (perhaps following an upgrade guide), their changes are lost. **Mitigation**: docs explicitly warn about `--force`. v0 considers this acceptable; a future migration story handles it better.
- **Symlinked target paths.** If `PATH` points at a symlink, behavior depends on `std::fs::canonicalize` semantics. **Mitigation**: don't canonicalize; resolve relative paths only via `Path::join`. The user's filesystem layout decides what gets written.
- **Race against concurrent `rupu init`.** Two parallel inits in the same dir could see partial state. **Mitigation**: not a real-world concern (init is a one-time operation); not worth lock-file complexity.

## 14. Success criteria

- `rupu init` in an empty directory creates the skeleton + `.gitignore` and exits 0.
- `rupu init --with-samples` additionally seeds 6 agents + 1 workflow with byte-equivalent content to the rupu repo's `.rupu/` files.
- `rupu init --with-samples` re-run in the same directory reports all files SKIPPED (idempotent).
- `rupu init --with-samples --force` overwrites all template files and reports them as OVERWROTE.
- `rupu init --git` in a non-repo creates `.git/`; in a repo, no-op.
- `cargo test -p rupu-cli` covers the matrix above.
- After `rupu init --with-samples` from a fresh directory + `rupu auth login`, `rupu run review-diff` works without further setup.
- `cargo build --release --workspace` and the workspace gates (fmt, clippy, test) all green.

## 15. Release vehicle

**v0.3.0**. Slice B-2 shipped under v0.2.0; B-3 is a meaningful UX addition that justifies its own minor. `cargo install --git ...` users see a clear release-note story.
