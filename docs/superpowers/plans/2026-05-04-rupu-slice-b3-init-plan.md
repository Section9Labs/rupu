# rupu Slice B-3 — `rupu init` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship `rupu init [PATH] [--with-samples] [--force] [--git]` — a single command that bootstraps `.rupu/agents/`, `.rupu/workflows/`, `.rupu/config.toml`, and a `.gitignore` entry, optionally seeding 6 curated agent templates + 1 workflow template embedded via `include_str!`.

**Architecture:** One new submodule (`crates/rupu-cli/src/cmd/init.rs`) plus a manifest module (`crates/rupu-cli/src/templates.rs`). Source-of-truth template files live under `crates/rupu-cli/templates/` and are embedded at build time. No new crate, no new workspace deps. Pure stdlib + `which` (already a workspace dep) for git-detection.

**Tech Stack:** Rust 2021 (MSRV 1.88), `clap` (existing), `std::fs`, `std::process::Command` for `git`, `which` for `git`-on-PATH detection.

**Spec:** `docs/superpowers/specs/2026-05-04-rupu-slice-b3-init-design.md`

---

## File Structure

```
crates/rupu-cli/
  Cargo.toml                                    # MODIFY: maybe `which.workspace = true` if not already
  src/
    cmd/
      init.rs                                   # NEW: subcommand handler (~250 lines incl. tests-public helpers)
      mod.rs                                    # MODIFY: pub mod init;
    templates.rs                                # NEW: include_str! manifest + content constants
    lib.rs                                      # MODIFY: Init(InitArgs) variant on Cmd; dispatcher arm
  templates/                                    # NEW directory — source-of-truth for embedded files
    agents/
      review-diff.md                            # byte-equivalent copy of .rupu/agents/review-diff.md
      add-tests.md                              # ditto
      fix-bug.md                                # ditto
      scaffold.md                               # ditto
      summarize-diff.md                         # ditto
      scm-pr-review.md                          # ditto
    workflows/
      investigate-then-fix.yaml                 # byte-equivalent copy of .rupu/workflows/investigate-then-fix.yaml
  tests/
    init_create_skeleton.rs                     # NEW
    init_with_samples.rs                        # NEW
    init_merge_behavior.rs                      # NEW
    init_force.rs                               # NEW
    init_gitignore.rs                           # NEW
    init_git_flag.rs                            # NEW
    init_manifest_in_sync.rs                    # NEW

README.md                                       # MODIFY: Quick-start section update
CHANGELOG.md                                    # MODIFY: v0.3.0 entry
docs/scm.md                                     # MODIFY: cross-reference rupu init
CLAUDE.md                                       # MODIFY: bump 9 → 10 subcommands; add `init`
```

## Conventions to honor

- Workspace deps only — `which` already exists in `[workspace.dependencies]` from earlier slices; verify before referencing.
- `#![deny(clippy::all)]` at every crate root (already in place).
- `unsafe_code` forbidden.
- Per the "no mock features" memory: every code path either writes the real template content or returns an explicit error. No silent fallbacks.
- Per the "no comments unless explaining WHY" rule: only annotate non-obvious decisions (e.g. why `.gitignore` is the only file rupu touches outside `.rupu/`).

## Important pre-existing state (read before starting)

- `crates/rupu-cli/src/cmd/mod.rs` declares the existing subcommand modules (`agent`, `auth`, `config`, `cron`, `mcp`, `models`, `repos`, `run`, `transcript`, `webhook`, `workflow`). `init` will join the list.
- `crates/rupu-cli/src/lib.rs` has the `Cmd` enum + dispatcher; `Init(InitArgs)` is added near `Repos`/`Mcp`.
- Today's sample agents/workflows live at `<repo>/.rupu/agents/` and `<repo>/.rupu/workflows/`. The four `sample-<provider>.md` files are test fixtures and are EXCLUDED from `--with-samples`. Two extra workflows (`code-review-panel.yaml`, `review-each-file.yaml`) exist but are deferred to follow-up curation; v0 ships only `investigate-then-fix.yaml`.
- `crates/rupu-cli/Cargo.toml` already depends on `anyhow`, `clap`, `tempfile` (now in `[dependencies]` from Plan 3). Confirm `which` is available.
- `crates/rupu-cli/src/cmd/repos.rs` is the canonical pattern for a small `cmd::*` subcommand: `Action` enum + `handle(action) -> ExitCode` + `*_inner()` returning `anyhow::Result<()>`.

---

## Phase 0 — Templates: copy source files into the new directory

### Task 1: Copy curated agent + workflow files into `crates/rupu-cli/templates/`

**Files:**
- Create: `crates/rupu-cli/templates/agents/review-diff.md`
- Create: `crates/rupu-cli/templates/agents/add-tests.md`
- Create: `crates/rupu-cli/templates/agents/fix-bug.md`
- Create: `crates/rupu-cli/templates/agents/scaffold.md`
- Create: `crates/rupu-cli/templates/agents/summarize-diff.md`
- Create: `crates/rupu-cli/templates/agents/scm-pr-review.md`
- Create: `crates/rupu-cli/templates/workflows/investigate-then-fix.yaml`

- [ ] **Step 1: Create the directory tree**

```bash
cd /Users/matt/Code/Oracle/rupu
mkdir -p crates/rupu-cli/templates/agents
mkdir -p crates/rupu-cli/templates/workflows
```

- [ ] **Step 2: Copy each source file into `templates/`**

```bash
cp .rupu/agents/review-diff.md     crates/rupu-cli/templates/agents/review-diff.md
cp .rupu/agents/add-tests.md       crates/rupu-cli/templates/agents/add-tests.md
cp .rupu/agents/fix-bug.md         crates/rupu-cli/templates/agents/fix-bug.md
cp .rupu/agents/scaffold.md        crates/rupu-cli/templates/agents/scaffold.md
cp .rupu/agents/summarize-diff.md  crates/rupu-cli/templates/agents/summarize-diff.md
cp .rupu/agents/scm-pr-review.md   crates/rupu-cli/templates/agents/scm-pr-review.md
cp .rupu/workflows/investigate-then-fix.yaml crates/rupu-cli/templates/workflows/investigate-then-fix.yaml
```

- [ ] **Step 3: Verify byte equivalence**

```bash
diff -r .rupu/agents/review-diff.md     crates/rupu-cli/templates/agents/review-diff.md
diff -r .rupu/agents/add-tests.md       crates/rupu-cli/templates/agents/add-tests.md
diff -r .rupu/agents/fix-bug.md         crates/rupu-cli/templates/agents/fix-bug.md
diff -r .rupu/agents/scaffold.md        crates/rupu-cli/templates/agents/scaffold.md
diff -r .rupu/agents/summarize-diff.md  crates/rupu-cli/templates/agents/summarize-diff.md
diff -r .rupu/agents/scm-pr-review.md   crates/rupu-cli/templates/agents/scm-pr-review.md
diff -r .rupu/workflows/investigate-then-fix.yaml crates/rupu-cli/templates/workflows/investigate-then-fix.yaml
```

Expected: zero output for each.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-cli/templates/
git commit -m "$(cat <<'EOF'
rupu-cli: add curated template sources for `rupu init --with-samples`

Six agent templates (review-diff, add-tests, fix-bug, scaffold,
summarize-diff, scm-pr-review) plus one workflow template
(investigate-then-fix). Byte-equivalent copies of the dogfooded
.rupu/ files in the rupu repo. Slice B-3 Task 2 wires them into
templates.rs via include_str!.

The four `sample-<provider>.md` agents in .rupu/ are test fixtures
and are deliberately excluded from this curated set.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 1 — Manifest module

### Task 2: Create `crates/rupu-cli/src/templates.rs`

**Files:**
- Create: `crates/rupu-cli/src/templates.rs`
- Modify: `crates/rupu-cli/src/lib.rs`

- [ ] **Step 1: Write `crates/rupu-cli/src/templates.rs`**

```rust
//! Embedded templates for `rupu init --with-samples`.
//!
//! The manifest is the single source-of-truth for what ships in
//! `--with-samples`. Adding a template is two steps:
//!
//!   1. Drop the file under `crates/rupu-cli/templates/<dir>/<name>`.
//!   2. Add a line to `MANIFEST` below.
//!
//! `init_manifest_in_sync.rs` enforces both directions: every file
//! under templates/ appears in MANIFEST, and every MANIFEST entry
//! exists on disk.

/// One curated template: a target-relative path (always under
/// `.rupu/`) and the embedded file content.
pub struct Template {
    /// Path RELATIVE to the project root, e.g. `.rupu/agents/review-diff.md`.
    pub target_relpath: &'static str,
    /// Raw file bytes embedded at build time via `include_str!`.
    pub content: &'static str,
}

/// The curated set shipped by `rupu init --with-samples`.
///
/// Test fixtures (`sample-<provider>.md` etc.) are intentionally NOT
/// in this list — they live in the rupu repo's `.rupu/` for slice
/// B-1 / B-2 development and are not user-facing templates.
pub const MANIFEST: &[Template] = &[
    Template {
        target_relpath: ".rupu/agents/review-diff.md",
        content: include_str!("../templates/agents/review-diff.md"),
    },
    Template {
        target_relpath: ".rupu/agents/add-tests.md",
        content: include_str!("../templates/agents/add-tests.md"),
    },
    Template {
        target_relpath: ".rupu/agents/fix-bug.md",
        content: include_str!("../templates/agents/fix-bug.md"),
    },
    Template {
        target_relpath: ".rupu/agents/scaffold.md",
        content: include_str!("../templates/agents/scaffold.md"),
    },
    Template {
        target_relpath: ".rupu/agents/summarize-diff.md",
        content: include_str!("../templates/agents/summarize-diff.md"),
    },
    Template {
        target_relpath: ".rupu/agents/scm-pr-review.md",
        content: include_str!("../templates/agents/scm-pr-review.md"),
    },
    Template {
        target_relpath: ".rupu/workflows/investigate-then-fix.yaml",
        content: include_str!("../templates/workflows/investigate-then-fix.yaml"),
    },
];

/// Skeleton config.toml content. Created on every `rupu init`.
pub const CONFIG_SKELETON: &str = r#"# rupu project config — see https://github.com/Section9Labs/rupu/blob/main/docs/providers.md

# default_model = "claude-sonnet-4-6"

# [scm.default]
# platform = "github"
# owner = "<your-org>"
# repo = "<this-repo>"

# [issues.default]
# tracker = "github"
# project = "<your-org>/<this-repo>"
"#;

/// `.gitignore` line that rupu owns. Init appends this to an existing
/// `.gitignore` (or creates one) when missing.
pub const GITIGNORE_ENTRY: &str = ".rupu/transcripts/";
```

That ENTIRE block is the file body, end-to-end. Note that `CONFIG_SKELETON` uses a single-`#` raw-string (`r#"..."#`) — that's fine because the body has no `"#` sequence inside. If an implementer accidentally double-fences it (`r##"..."##`) Rust still accepts; either is safe.

- [ ] **Step 2: Re-export from `lib.rs`**

In `crates/rupu-cli/src/lib.rs`, find the existing `pub mod ...;` declarations near the top and add:

```rust
pub mod templates;
```

(Place it alphabetically with the other `pub mod` lines — likely between `paths` and any module starting with letters after `t`.)

- [ ] **Step 3: Verify it compiles**

```bash
cd /Users/matt/Code/Oracle/rupu
cargo build -p rupu-cli 2>&1 | tail -5
```

Expected: clean compile. The seven `include_str!` calls each resolve to a real file from Task 1.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-cli/src/templates.rs crates/rupu-cli/src/lib.rs
git commit -m "$(cat <<'EOF'
rupu-cli: templates manifest module + include_str! bindings

Single source-of-truth for which agents + workflows ship via
`rupu init --with-samples`. Adding a template is two steps:
drop the file under crates/rupu-cli/templates/<dir>/, add a
manifest entry. The init_manifest_in_sync test (Task 9) ensures
both directions stay aligned.

Also exposes CONFIG_SKELETON and GITIGNORE_ENTRY constants used
by the skeleton-creation path in `cmd::init`.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 2 — Subcommand handler

### Task 3: Create the `Init(InitArgs)` clap variant + module stub

**Files:**
- Create: `crates/rupu-cli/src/cmd/init.rs` (stub returning a clear error so wiring compiles)
- Modify: `crates/rupu-cli/src/cmd/mod.rs`
- Modify: `crates/rupu-cli/src/lib.rs`

- [ ] **Step 1: Create `crates/rupu-cli/src/cmd/init.rs` with a stub handler**

```rust
//! `rupu init [PATH] [--with-samples] [--force] [--git]` — bootstrap a
//! project's `.rupu/` directory.
//!
//! Spec: docs/superpowers/specs/2026-05-04-rupu-slice-b3-init-design.md

use clap::Args as ClapArgs;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(ClapArgs, Debug)]
pub struct InitArgs {
    /// Target directory for the new project. Defaults to the current
    /// working directory.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Include the curated agent + workflow templates.
    #[arg(long)]
    pub with_samples: bool,

    /// Overwrite existing template files (still merges by default).
    #[arg(long)]
    pub force: bool,

    /// Run `git init` afterwards if the target is not already inside a git repo.
    #[arg(long)]
    pub git: bool,
}

pub async fn handle(args: InitArgs) -> ExitCode {
    match init_inner(args) {
        Ok(()) => ExitCode::from(0),
        Err(e) => {
            eprintln!("rupu init: {e}");
            ExitCode::from(1)
        }
    }
}

fn init_inner(_args: InitArgs) -> anyhow::Result<()> {
    anyhow::bail!("not yet implemented (Task 4)")
}
```

- [ ] **Step 2: Add `pub mod init;` to `cmd/mod.rs`**

In `crates/rupu-cli/src/cmd/mod.rs`, insert `pub mod init;` alphabetically. After Slice B-2's additions the file looks like:

```rust
//! Subcommand handlers. Each module owns one verb.

pub mod agent;
pub mod auth;
pub mod config;
pub mod cron;
pub mod init;        // <-- new
pub mod mcp;
pub mod models;
pub mod repos;
pub mod run;
pub mod transcript;
pub mod webhook;
pub mod workflow;
```

- [ ] **Step 3: Wire `Init { args }` into the `Cmd` enum + dispatcher**

In `crates/rupu-cli/src/lib.rs`, add to the `Cmd` enum (alphabetically near `Mcp` / `Models`):

```rust
    /// Bootstrap a new rupu project (`.rupu/agents`, `.rupu/workflows`, config).
    Init(cmd::init::InitArgs),
```

Note `InitArgs` is a flat `ClapArgs` struct (not a `Subcommand` enum), so the variant uses tuple syntax `Init(InitArgs)` — matching `Cmd::Run(args)` pattern, NOT the `Cmd::Mcp { action }` pattern.

In the dispatcher `match cli.command { ... }`, add:

```rust
        Cmd::Init(args) => cmd::init::handle(args).await,
```

- [ ] **Step 4: Verify clap parses the new subcommand**

```bash
cd /Users/matt/Code/Oracle/rupu
cargo build -p rupu-cli 2>&1 | tail -5
cargo run -p rupu-cli -- init --help 2>&1 | head -25
```

Expected output includes:
```
Bootstrap a new rupu project (.rupu/agents, .rupu/workflows, config).

Usage: rupu init [OPTIONS] [PATH]

Arguments:
  [PATH]  Target directory for the new project ... [default: .]

Options:
      --with-samples  Include the curated agent + workflow templates
      --force         Overwrite existing template files ...
      --git           Run `git init` afterwards ...
```

- [ ] **Step 5: Confirm the stub returns a clear "not yet implemented" error**

```bash
cargo run -p rupu-cli -- init /tmp/rupu-init-stub 2>&1 | head -3
```

Expected:
```
rupu init: not yet implemented (Task 4)
```
Exit code 1.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cli/src/cmd/init.rs crates/rupu-cli/src/cmd/mod.rs crates/rupu-cli/src/lib.rs
git commit -m "$(cat <<'EOF'
rupu-cli: clap wiring for `rupu init` (stub handler)

InitArgs has four fields per spec §4: path (default "."), --with-samples,
--force, --git. Handler is a stub returning "not yet implemented (Task 4)";
the next task fills in skeleton creation, then merge behavior, then --git.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Implement skeleton creation (no samples yet)

**Files:**
- Modify: `crates/rupu-cli/src/cmd/init.rs`
- Create: `crates/rupu-cli/tests/init_create_skeleton.rs`

- [ ] **Step 1: Write the failing integration test**

Create `crates/rupu-cli/tests/init_create_skeleton.rs`:

```rust
//! `rupu init` against an empty TempDir creates the skeleton:
//!   .rupu/agents/, .rupu/workflows/, .rupu/config.toml, and a
//!   .gitignore with the transcripts entry.

use std::path::Path;

use rupu_cli::cmd::init::{init_for_test, InitArgs};

fn args(path: &Path) -> InitArgs {
    InitArgs {
        path: path.to_path_buf(),
        with_samples: false,
        force: false,
        git: false,
    }
}

#[test]
fn empty_dir_gets_full_skeleton() {
    let tmp = tempfile::tempdir().unwrap();

    init_for_test(args(tmp.path())).expect("init should succeed");

    assert!(tmp.path().join(".rupu").is_dir());
    assert!(tmp.path().join(".rupu/agents").is_dir());
    assert!(tmp.path().join(".rupu/workflows").is_dir());

    let cfg = std::fs::read_to_string(tmp.path().join(".rupu/config.toml")).unwrap();
    assert!(cfg.contains("rupu project config"));
    assert!(cfg.contains("[scm.default]"));

    let gi = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
    assert!(gi.contains(".rupu/transcripts/"));
}
```

- [ ] **Step 2: Run, verify it fails to compile**

```bash
cd /Users/matt/Code/Oracle/rupu
cargo test -p rupu-cli --test init_create_skeleton 2>&1 | tail -10
```

Expected: compile error — `init_for_test` does not exist.

- [ ] **Step 3: Implement `init_for_test` + skeleton logic in `cmd/init.rs`**

Replace the `init_inner` body and add a `pub` testable wrapper. Final state of `crates/rupu-cli/src/cmd/init.rs`:

```rust
//! `rupu init [PATH] [--with-samples] [--force] [--git]` — bootstrap a
//! project's `.rupu/` directory.
//!
//! Spec: docs/superpowers/specs/2026-05-04-rupu-slice-b3-init-design.md

use clap::Args as ClapArgs;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::templates::{CONFIG_SKELETON, GITIGNORE_ENTRY};

#[derive(ClapArgs, Debug)]
pub struct InitArgs {
    /// Target directory for the new project. Defaults to the current
    /// working directory.
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Include the curated agent + workflow templates.
    #[arg(long)]
    pub with_samples: bool,

    /// Overwrite existing template files (still merges by default).
    #[arg(long)]
    pub force: bool,

    /// Run `git init` afterwards if the target is not already inside a git repo.
    #[arg(long)]
    pub git: bool,
}

pub async fn handle(args: InitArgs) -> ExitCode {
    match init_inner(args) {
        Ok(()) => ExitCode::from(0),
        Err(e) => {
            eprintln!("rupu init: {e}");
            ExitCode::from(1)
        }
    }
}

/// Test entry point. Same code path as `handle` but returns the error
/// instead of mapping to ExitCode, so integration tests can assert on
/// success/failure without spawning a binary.
pub fn init_for_test(args: InitArgs) -> anyhow::Result<()> {
    init_inner(args)
}

fn init_inner(args: InitArgs) -> anyhow::Result<()> {
    let root = &args.path;
    if !root.exists() {
        fs::create_dir_all(root)?;
    } else if !root.is_dir() {
        anyhow::bail!("PATH exists but is not a directory: {}", root.display());
    }

    create_skeleton(root)?;
    ensure_gitignore_entry(root)?;
    Ok(())
}

fn create_skeleton(root: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(root.join(".rupu/agents"))?;
    fs::create_dir_all(root.join(".rupu/workflows"))?;

    let cfg_path = root.join(".rupu/config.toml");
    if !cfg_path.exists() {
        fs::write(&cfg_path, CONFIG_SKELETON)?;
        println!("CREATED {}", relpath(root, &cfg_path));
    } else {
        println!("SKIPPED {} (exists)", relpath(root, &cfg_path));
    }
    Ok(())
}

fn ensure_gitignore_entry(root: &Path) -> anyhow::Result<()> {
    let path = root.join(".gitignore");
    let needle = GITIGNORE_ENTRY;

    if !path.exists() {
        fs::write(&path, format!("{needle}\n"))?;
        println!("CREATED {}", relpath(root, &path));
        return Ok(());
    }

    let body = fs::read_to_string(&path)?;
    if body.lines().any(|l| l.trim() == needle) {
        return Ok(());
    }
    let mut new_body = body;
    if !new_body.ends_with('\n') {
        new_body.push('\n');
    }
    new_body.push_str(needle);
    new_body.push('\n');
    fs::write(&path, new_body)?;
    println!("UPDATED {} (appended {needle})", relpath(root, &path));
    Ok(())
}

fn relpath(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .display()
        .to_string()
}
```

- [ ] **Step 4: Re-run the test**

```bash
cargo test -p rupu-cli --test init_create_skeleton 2>&1 | tail -10
```

Expected: `1 passed; 0 failed`.

- [ ] **Step 5: Verify gates**

```bash
cargo fmt -p rupu-cli -- --check
cargo clippy -p rupu-cli --all-targets -- -D warnings 2>&1 | tail -5
```

Both clean.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cli/src/cmd/init.rs crates/rupu-cli/tests/init_create_skeleton.rs
git commit -m "$(cat <<'EOF'
rupu-cli: rupu init creates skeleton (no samples yet)

Bare `rupu init` creates .rupu/{agents,workflows}/, writes a
commented config.toml skeleton, and ensures .gitignore contains
the .rupu/transcripts/ line. Idempotent: re-runs are no-ops on
already-present files.

Exposes init_for_test() so integration tests don't need to spawn
the binary.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: Implement `--with-samples` (manifest writes)

**Files:**
- Modify: `crates/rupu-cli/src/cmd/init.rs`
- Create: `crates/rupu-cli/tests/init_with_samples.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-cli/tests/init_with_samples.rs`:

```rust
//! `rupu init --with-samples` writes every entry in MANIFEST and the
//! content matches the embedded source byte-for-byte.

use std::path::Path;

use rupu_cli::cmd::init::{init_for_test, InitArgs};
use rupu_cli::templates::MANIFEST;

fn args(path: &Path) -> InitArgs {
    InitArgs {
        path: path.to_path_buf(),
        with_samples: true,
        force: false,
        git: false,
    }
}

#[test]
fn with_samples_seeds_every_manifest_entry() {
    let tmp = tempfile::tempdir().unwrap();
    init_for_test(args(tmp.path())).unwrap();

    for t in MANIFEST {
        let p = tmp.path().join(t.target_relpath);
        assert!(p.exists(), "missing template file: {}", t.target_relpath);
        let body = std::fs::read_to_string(&p).unwrap();
        assert_eq!(
            body, t.content,
            "content mismatch for {}",
            t.target_relpath
        );
    }
}

#[test]
fn samples_byte_match_dogfooded_files() {
    // Catches drift between crates/rupu-cli/templates/* and the
    // .rupu/* files in the rupu repo. If this fails, copy the
    // newer one over the older.
    for t in MANIFEST {
        let on_disk = std::fs::read_to_string(t.target_relpath).unwrap_or_else(|e| {
            panic!(
                "could not read dogfooded source {}: {e}",
                t.target_relpath
            )
        });
        assert_eq!(
            on_disk, t.content,
            "drift between {} (rupu repo) and the embedded template",
            t.target_relpath
        );
    }
}
```

(Test 2 reads relative to the package's working directory at test time; cargo runs tests from the crate root, so `.rupu/agents/review-diff.md` resolves to `<workspace_root>/.rupu/agents/review-diff.md`. If the test crate's CWD differs in some CI configurations, this assertion may need an env-var override — verify in Step 4 below.)

- [ ] **Step 2: Run, verify failure**

```bash
cd /Users/matt/Code/Oracle/rupu
cargo test -p rupu-cli --test init_with_samples 2>&1 | tail -10
```

Expected: tests fail because `init_inner` ignores `with_samples`.

- [ ] **Step 3: Extend `init_inner` to write manifest entries**

In `crates/rupu-cli/src/cmd/init.rs`, modify `init_inner` and add a helper:

```rust
fn init_inner(args: InitArgs) -> anyhow::Result<()> {
    let root = &args.path;
    if !root.exists() {
        fs::create_dir_all(root)?;
    } else if !root.is_dir() {
        anyhow::bail!("PATH exists but is not a directory: {}", root.display());
    }

    let mut tally = WriteTally::default();
    create_skeleton(root, &mut tally)?;
    ensure_gitignore_entry(root)?;

    if args.with_samples {
        write_manifest(root, args.force, &mut tally)?;
    }

    println!(
        "init: created {}, skipped {}, overwrote {}",
        tally.created, tally.skipped, tally.overwrote
    );
    Ok(())
}

#[derive(Default)]
struct WriteTally {
    created: usize,
    skipped: usize,
    overwrote: usize,
}

fn write_manifest(root: &Path, force: bool, tally: &mut WriteTally) -> anyhow::Result<()> {
    use crate::templates::MANIFEST;
    for t in MANIFEST {
        let dest = root.join(t.target_relpath);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let action = write_file(&dest, t.content, force)?;
        match action {
            FileAction::Created => {
                println!("CREATED {}", relpath(root, &dest));
                tally.created += 1;
            }
            FileAction::Skipped => {
                println!("SKIPPED {} (exists)", relpath(root, &dest));
                tally.skipped += 1;
            }
            FileAction::Overwrote => {
                println!("OVERWROTE {}", relpath(root, &dest));
                tally.overwrote += 1;
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum FileAction {
    Created,
    Skipped,
    Overwrote,
}

fn write_file(path: &Path, content: &str, force: bool) -> anyhow::Result<FileAction> {
    if !path.exists() {
        fs::write(path, content)?;
        return Ok(FileAction::Created);
    }
    if force {
        fs::write(path, content)?;
        return Ok(FileAction::Overwrote);
    }
    Ok(FileAction::Skipped)
}
```

Update `create_skeleton` to take `&mut WriteTally` so the config.toml line counts:

```rust
fn create_skeleton(root: &Path, tally: &mut WriteTally) -> anyhow::Result<()> {
    fs::create_dir_all(root.join(".rupu/agents"))?;
    fs::create_dir_all(root.join(".rupu/workflows"))?;

    let cfg_path = root.join(".rupu/config.toml");
    let action = write_file(&cfg_path, CONFIG_SKELETON, false)?;
    match action {
        FileAction::Created => {
            println!("CREATED {}", relpath(root, &cfg_path));
            tally.created += 1;
        }
        FileAction::Skipped => {
            println!("SKIPPED {} (exists)", relpath(root, &cfg_path));
            tally.skipped += 1;
        }
        FileAction::Overwrote => unreachable!("config.toml never gets force=true at this layer"),
    }
    Ok(())
}
```

(`config.toml` does NOT honor `--force` — it's a small commented skeleton, and overwriting a customized config is a worse footgun than the agents. Only manifest files honor `--force`. Document this with a one-line comment near `create_skeleton`.)

- [ ] **Step 4: Re-run tests**

```bash
cargo test -p rupu-cli --test init_with_samples 2>&1 | tail -10
```

Expected: 2 passed.

If `samples_byte_match_dogfooded_files` fails because cargo runs tests from a different CWD, add `let workspace_root = env!("CARGO_MANIFEST_DIR");` and join with `../../<relpath>` — i.e. read from `<workspace_root>/../../.rupu/agents/...`. Verify the path resolves correctly.

- [ ] **Step 5: Verify gates**

```bash
cargo fmt -p rupu-cli -- --check
cargo clippy -p rupu-cli --all-targets -- -D warnings 2>&1 | tail -5
```

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cli/src/cmd/init.rs crates/rupu-cli/tests/init_with_samples.rs
git commit -m "$(cat <<'EOF'
rupu-cli: --with-samples writes the manifest

Iterates templates::MANIFEST, writing each entry under .rupu/<path>.
Reports per-file action (CREATED / SKIPPED / OVERWROTE) and a
final tally line. config.toml uses the same write_file helper but
never honors --force at this layer (overwriting a customized
config is a worse footgun than the agents).

Tests assert content byte-equivalence between the embedded
templates and the dogfooded .rupu/ files in the rupu repo.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 6: Tests for merge / `--force` behavior

**Files:**
- Create: `crates/rupu-cli/tests/init_merge_behavior.rs`
- Create: `crates/rupu-cli/tests/init_force.rs`

- [ ] **Step 1: Write `init_merge_behavior.rs`**

```rust
//! Pre-existing template files are SKIPPED by default.

use rupu_cli::cmd::init::{init_for_test, InitArgs};

#[test]
fn pre_existing_template_is_skipped_default() {
    let tmp = tempfile::tempdir().unwrap();
    let agents = tmp.path().join(".rupu/agents");
    std::fs::create_dir_all(&agents).unwrap();
    let stub = "MY CUSTOM AGENT — DO NOT TOUCH\n";
    std::fs::write(agents.join("review-diff.md"), stub).unwrap();

    init_for_test(InitArgs {
        path: tmp.path().to_path_buf(),
        with_samples: true,
        force: false,
        git: false,
    })
    .unwrap();

    let body = std::fs::read_to_string(agents.join("review-diff.md")).unwrap();
    assert_eq!(body, stub, "pre-existing file must NOT be overwritten");

    // Other templates should still be created.
    assert!(agents.join("add-tests.md").exists());
}
```

- [ ] **Step 2: Write `init_force.rs`**

```rust
//! `--force` overwrites pre-existing template files.

use rupu_cli::cmd::init::{init_for_test, InitArgs};
use rupu_cli::templates::MANIFEST;

#[test]
fn force_overwrites_existing_templates() {
    let tmp = tempfile::tempdir().unwrap();
    let target = tmp.path().join(".rupu/agents/review-diff.md");
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, "stub\n").unwrap();

    init_for_test(InitArgs {
        path: tmp.path().to_path_buf(),
        with_samples: true,
        force: true,
        git: false,
    })
    .unwrap();

    let expected = MANIFEST
        .iter()
        .find(|t| t.target_relpath.ends_with("review-diff.md"))
        .unwrap()
        .content;
    let body = std::fs::read_to_string(&target).unwrap();
    assert_eq!(body, expected, "--force must overwrite with template content");
}
```

- [ ] **Step 3: Run both tests**

```bash
cd /Users/matt/Code/Oracle/rupu
cargo test -p rupu-cli --test init_merge_behavior 2>&1 | tail -5
cargo test -p rupu-cli --test init_force 2>&1 | tail -5
```

Expected: 1 passed in each.

- [ ] **Step 4: Commit**

```bash
git add crates/rupu-cli/tests/init_merge_behavior.rs crates/rupu-cli/tests/init_force.rs
git commit -m "$(cat <<'EOF'
rupu-cli: tests pin merge + --force semantics for `rupu init`

Default is merge-by-default: pre-existing template files are
SKIPPED. --force overwrites pre-existing template files with the
embedded content. Both behaviors covered by integration tests
that pre-create a stub then run init_for_test().

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 7: `.gitignore` test matrix

**Files:**
- Create: `crates/rupu-cli/tests/init_gitignore.rs`

- [ ] **Step 1: Write the test**

```rust
//! Gitignore handling: missing → created; present without entry → appended;
//! present with entry → unchanged.

use rupu_cli::cmd::init::{init_for_test, InitArgs};

fn run(path: &std::path::Path) {
    init_for_test(InitArgs {
        path: path.to_path_buf(),
        with_samples: false,
        force: false,
        git: false,
    })
    .unwrap();
}

#[test]
fn missing_gitignore_is_created() {
    let tmp = tempfile::tempdir().unwrap();
    run(tmp.path());
    let body = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
    assert!(body.contains(".rupu/transcripts/"));
}

#[test]
fn pre_existing_gitignore_without_entry_is_appended() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(".gitignore"), "/target\nnode_modules/\n").unwrap();
    run(tmp.path());
    let body = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
    assert!(body.contains("/target"), "pre-existing entries preserved");
    assert!(body.contains("node_modules/"));
    assert!(body.contains(".rupu/transcripts/"), "rupu entry appended");
}

#[test]
fn pre_existing_gitignore_with_entry_is_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let original = "/target\n.rupu/transcripts/\nnode_modules/\n";
    std::fs::write(tmp.path().join(".gitignore"), original).unwrap();
    run(tmp.path());
    let body = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
    assert_eq!(body, original, "no change when entry already present");
}

#[test]
fn pre_existing_gitignore_no_trailing_newline_is_handled() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join(".gitignore"), "/target").unwrap(); // no trailing \n
    run(tmp.path());
    let body = std::fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
    // Should be "/target\n.rupu/transcripts/\n" — both lines well-formed.
    assert!(body.starts_with("/target\n"));
    assert!(body.contains(".rupu/transcripts/"));
    assert!(body.ends_with('\n'));
}
```

- [ ] **Step 2: Run**

```bash
cd /Users/matt/Code/Oracle/rupu
cargo test -p rupu-cli --test init_gitignore 2>&1 | tail -10
```

Expected: 4 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-cli/tests/init_gitignore.rs
git commit -m "$(cat <<'EOF'
rupu-cli: gitignore handling test matrix

Four cases: missing file (create), present-without-entry (append),
present-with-entry (unchanged), present-no-trailing-newline (append
with leading newline). All idempotent — running init twice never
adds a duplicate `.rupu/transcripts/` line.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 8: `--git` flag

**Files:**
- Modify: `crates/rupu-cli/src/cmd/init.rs`
- Create: `crates/rupu-cli/tests/init_git_flag.rs`

- [ ] **Step 1: Write the test**

```rust
//! `--git` runs git init on a non-repo dir and is a no-op on a repo.

use rupu_cli::cmd::init::{init_for_test, InitArgs};

fn run_git(path: &std::path::Path) {
    init_for_test(InitArgs {
        path: path.to_path_buf(),
        with_samples: false,
        force: false,
        git: true,
    })
    .unwrap();
}

#[test]
fn git_flag_inits_git_in_empty_dir() {
    if which::which("git").is_err() {
        eprintln!("skipping: git not on PATH");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    run_git(tmp.path());
    assert!(tmp.path().join(".git").exists(), ".git/ should be created");
}

#[test]
fn git_flag_is_noop_in_existing_repo() {
    if which::which("git").is_err() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    std::process::Command::new("git")
        .arg("init")
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let head_before = std::fs::read_to_string(tmp.path().join(".git/HEAD")).unwrap();
    run_git(tmp.path());
    let head_after = std::fs::read_to_string(tmp.path().join(".git/HEAD")).unwrap();
    assert_eq!(head_before, head_after, "second init should be no-op");
}
```

- [ ] **Step 2: Run, verify failure**

```bash
cd /Users/matt/Code/Oracle/rupu
cargo test -p rupu-cli --test init_git_flag 2>&1 | tail -10
```

Expected: tests fail because `init_inner` ignores `args.git`.

- [ ] **Step 3: Implement the `--git` branch in `init.rs`**

Append to `init_inner`, after the `tally` print line:

```rust
    if args.git {
        maybe_git_init(root)?;
    }
    Ok(())
}

fn maybe_git_init(root: &Path) -> anyhow::Result<()> {
    if which::which("git").is_err() {
        eprintln!("init: --git requested but git not found on PATH; skipping");
        return Ok(());
    }
    let inside = std::process::Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(root)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .map(|o| o.status.success() && o.stdout.starts_with(b"true"))
        .unwrap_or(false);
    if inside {
        return Ok(());
    }
    let status = std::process::Command::new("git")
        .arg("init")
        .current_dir(root)
        .status()?;
    if !status.success() {
        eprintln!("init: git init exited with status {status}; continuing");
    }
    Ok(())
}
```

Add `which.workspace = true` to `crates/rupu-cli/Cargo.toml`'s `[dependencies]` if not already present. Verify by reading the file first; if `which` is already in `[workspace.dependencies]` but not in this crate's deps, add the line; if `which` is not in workspace deps either, add `which = "6"` to root `Cargo.toml`'s `[workspace.dependencies]` first.

- [ ] **Step 4: Re-run the tests**

```bash
cargo test -p rupu-cli --test init_git_flag 2>&1 | tail -10
```

Expected: 2 passed.

- [ ] **Step 5: Verify gates**

```bash
cargo fmt -p rupu-cli -- --check
cargo clippy -p rupu-cli --all-targets -- -D warnings 2>&1 | tail -5
```

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-cli/src/cmd/init.rs \
        crates/rupu-cli/tests/init_git_flag.rs \
        crates/rupu-cli/Cargo.toml \
        Cargo.toml Cargo.lock
git commit -m "$(cat <<'EOF'
rupu-cli: --git flag runs git init on non-repo dirs

Detects "already in a git repo" via `git rev-parse --is-inside-work-tree`
and is a no-op in that case. Missing `git` on PATH is a stderr
warning, not a hard error — the file writes already succeeded and
that's the more important step.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 9: Manifest-sync test

**Files:**
- Create: `crates/rupu-cli/tests/init_manifest_in_sync.rs`

- [ ] **Step 1: Write the test**

```rust
//! Bidirectional manifest sync:
//!   1. Every entry in MANIFEST exists on disk under crates/rupu-cli/templates/.
//!   2. Every file under crates/rupu-cli/templates/ appears in MANIFEST.

use std::collections::HashSet;
use std::path::PathBuf;

use rupu_cli::templates::MANIFEST;

fn templates_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("templates")
}

#[test]
fn every_manifest_entry_exists_on_disk() {
    let dir = templates_dir();
    for t in MANIFEST {
        // target_relpath is `.rupu/agents/foo.md`; the on-disk source
        // is `templates/agents/foo.md` — drop the ".rupu/" prefix.
        let stripped = t
            .target_relpath
            .strip_prefix(".rupu/")
            .expect("manifest paths must start with .rupu/");
        let path = dir.join(stripped);
        assert!(
            path.exists(),
            "manifest entry {} has no source file at {}",
            t.target_relpath,
            path.display()
        );
    }
}

#[test]
fn every_template_file_is_in_manifest() {
    let dir = templates_dir();
    let mut on_disk = HashSet::new();
    walk(&dir, &dir, &mut on_disk);

    let in_manifest: HashSet<String> = MANIFEST
        .iter()
        .map(|t| {
            t.target_relpath
                .strip_prefix(".rupu/")
                .expect("manifest paths must start with .rupu/")
                .to_string()
        })
        .collect();

    let missing: Vec<&String> = on_disk.difference(&in_manifest).collect();
    assert!(
        missing.is_empty(),
        "files exist under templates/ but are not in MANIFEST: {missing:?}"
    );
}

fn walk(base: &std::path::Path, dir: &std::path::Path, out: &mut HashSet<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk(base, &p, out);
        } else if p.is_file() {
            let rel = p.strip_prefix(base).unwrap().display().to_string();
            out.insert(rel);
        }
    }
}
```

- [ ] **Step 2: Run**

```bash
cd /Users/matt/Code/Oracle/rupu
cargo test -p rupu-cli --test init_manifest_in_sync 2>&1 | tail -10
```

Expected: 2 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-cli/tests/init_manifest_in_sync.rs
git commit -m "$(cat <<'EOF'
rupu-cli: bidirectional manifest sync test

Catches the "file added on disk but forgotten in code" failure
mode AND the "manifest entry references a missing file" failure
mode. Future template additions will fail this test until they
update both halves.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

### Task 10: Binary smoke test

**Files:**
- Create: `crates/rupu-cli/tests/init_smoke.rs`

- [ ] **Step 1: Write the test**

```rust
//! Smoke test: spawn the actual rupu binary against a TempDir and
//! parse its stdout to confirm CREATED lines for every template plus
//! the final tally line.

use std::process::Command;

#[test]
fn rupu_init_with_samples_smoke() {
    let tmp = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_rupu"))
        .args([
            "init",
            "--with-samples",
            tmp.path().to_str().unwrap(),
        ])
        .output()
        .expect("spawn rupu");
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));

    let stdout = String::from_utf8_lossy(&output.stdout);
    for needle in [
        ".rupu/agents/review-diff.md",
        ".rupu/agents/scm-pr-review.md",
        ".rupu/workflows/investigate-then-fix.yaml",
    ] {
        assert!(stdout.contains(needle), "stdout missing {needle}:\n{stdout}");
    }
    assert!(stdout.contains("init: created"), "stdout missing tally line: {stdout}");

    // Spot-check a couple of files actually exist.
    assert!(tmp.path().join(".rupu/agents/review-diff.md").exists());
    assert!(tmp.path().join(".rupu/workflows/investigate-then-fix.yaml").exists());
    assert!(tmp.path().join(".rupu/config.toml").exists());
    assert!(tmp.path().join(".gitignore").exists());
}
```

- [ ] **Step 2: Run**

```bash
cd /Users/matt/Code/Oracle/rupu
cargo test -p rupu-cli --test init_smoke 2>&1 | tail -10
```

Expected: 1 passed.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-cli/tests/init_smoke.rs
git commit -m "$(cat <<'EOF'
rupu-cli: end-to-end smoke for `rupu init --with-samples`

Spawns the actual binary against a TempDir and asserts the
expected CREATED lines + tally appear on stdout. The matrix of
behavioral cases (merge / force / gitignore / git) lives in the
narrower integration tests; this one just confirms the binary's
exit path works end-to-end.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 3 — Documentation

### Task 11: README + CHANGELOG + docs/scm.md cross-ref + CLAUDE.md

**Files:**
- Modify: `README.md`
- Modify: `CHANGELOG.md`
- Modify: `docs/scm.md`
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update README quick-start**

Read the existing `README.md`. Find the section that today says something like "copy from the rupu repo" or otherwise describes seeding `.rupu/`. Replace with:

```markdown
## Quick start

```bash
# 1. Bootstrap a new project
rupu init --with-samples --git

# 2. Authenticate at least one provider
rupu auth login --provider anthropic --mode sso

# 3. Run an agent
rupu run review-diff
```

`rupu init --with-samples` seeds six curated agent templates
(`review-diff`, `add-tests`, `fix-bug`, `scaffold`, `summarize-diff`,
`scm-pr-review`) plus one workflow (`investigate-then-fix`) under
`.rupu/`. Re-running is a no-op; pass `--force` to overwrite local
template customizations with the latest embedded versions.
```

If a similar section already exists, edit in place rather than duplicate.

- [ ] **Step 2: Add a CHANGELOG v0.3.0 entry at the top**

```markdown
## v0.3.0 — Slice B-3: `rupu init` (2026-05-XX)

### Added

- **`rupu init [PATH] [--with-samples] [--force] [--git]`** bootstraps a
  project's `.rupu/` directory in one command.
- **Curated template set** (`--with-samples`): 6 agent templates
  (`review-diff`, `add-tests`, `fix-bug`, `scaffold`, `summarize-diff`,
  `scm-pr-review`) plus one workflow (`investigate-then-fix`), embedded
  via `include_str!` so `cargo install` users don't need network on
  first run.
- **`.gitignore`** auto-managed: `.rupu/transcripts/` is appended on
  init (idempotent).
- **`--git`** flag runs `git init` if the target isn't already in a
  repo. Missing `git` on PATH is a warning, not a hard error.

### Internal

- New module `crates/rupu-cli/src/templates.rs` with the manifest;
  bidirectional sync test ensures `templates/` and the manifest
  never drift apart.
- `rupu-cli` subcommand count: 9 → 10.
```

- [ ] **Step 3: Cross-reference from `docs/scm.md`**

Open `docs/scm.md`. Near the top, in the existing introduction or auth section, append a line like:

```markdown
> New project? Run `rupu init --with-samples` to seed `.rupu/agents/scm-pr-review.md` and the rest of the curated templates.
```

Place it where it reads naturally — likely right after the "## Auth" section's intro paragraph.

- [ ] **Step 4: Update CLAUDE.md**

In `CLAUDE.md`, find the rupu-cli crate description:

```markdown
- **`rupu-cli`** — the `rupu` binary. Thin clap dispatcher to the libraries. Nine subcommands: `run` / `agent` / `workflow` / `transcript` / `config` / `auth` / `models` / `repos` / `mcp`.
```

Bump to:

```markdown
- **`rupu-cli`** — the `rupu` binary. Thin clap dispatcher to the libraries. Ten subcommands: `init` / `run` / `agent` / `workflow` / `transcript` / `config` / `auth` / `models` / `repos` / `mcp`.
```

(Note: there's also `cron` and `webhook` from main — if the existing CLAUDE.md already lists 11 subcommands including those, just add `init` at the front and bump to 12. Match what the file actually says today.)

- [ ] **Step 5: Commit**

```bash
git add README.md CHANGELOG.md docs/scm.md CLAUDE.md
git commit -m "$(cat <<'EOF'
docs: README quick-start, CHANGELOG v0.3.0, scm.md cross-ref, CLAUDE.md

- README's quick-start now leads with `rupu init --with-samples --git`
  instead of the old "copy from rupu repo" workaround.
- CHANGELOG entry for v0.3.0 covering the new subcommand + curated
  template set.
- docs/scm.md cross-references init for new projects.
- CLAUDE.md bumps rupu-cli's subcommand count to include init.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 4 — Final gates

### Task 12: Workspace gates + binary smoke

**Files:**
- (none — verification only)

- [ ] **Step 1: Run all gates**

```bash
cd /Users/matt/Code/Oracle/rupu
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings 2>&1 | tail -10
cargo test -p rupu-cli --no-fail-fast 2>&1 | tail -25
```

All exit 0. The `init_*` test files all pass (~13 tests across 7 files including the smoke + manifest-sync).

If `cargo fmt` flags pre-existing drift in unrelated files, run `cargo fmt --all` and stage just the `crates/rupu-cli/` files in this task's commit.

- [ ] **Step 2: Smoke the release binary**

```bash
cargo build --release --workspace 2>&1 | tail -3
./target/release/rupu --version
./target/release/rupu --help                       # should list `init`
./target/release/rupu init --help                  # should match Task 3 Step 4 output
./target/release/rupu init /tmp/rupu-init-smoke --with-samples --git
ls /tmp/rupu-init-smoke/.rupu/agents/
ls /tmp/rupu-init-smoke/.rupu/workflows/
cat /tmp/rupu-init-smoke/.gitignore
test -d /tmp/rupu-init-smoke/.git && echo "git ok"
rm -rf /tmp/rupu-init-smoke
```

Confirm the agent + workflow files are present, `.gitignore` contains `.rupu/transcripts/`, and `.git` exists.

- [ ] **Step 3: No commit** — this task is verification only. If something failed, fix it inline and amend the commit from the relevant Task 4-11.

---

## Plan success criteria

After all 12 tasks complete:

- `cargo fmt --all -- --check` ✅
- `cargo clippy --workspace --all-targets -- -D warnings` ✅
- `cargo test -p rupu-cli` covers ~13 tests across 7 init_* files plus the smoke; all pass.
- `cargo build --release --workspace` produces a binary that lists `init` in `--help` and successfully bootstraps `/tmp/rupu-init-smoke` with all curated templates.
- The dogfooded `.rupu/` files in the rupu repo and the embedded `crates/rupu-cli/templates/` content are byte-equivalent (enforced by `init_with_samples.rs::samples_byte_match_dogfooded_files`).
- README's quick-start leads with `rupu init`; CHANGELOG has the v0.3.0 entry; CLAUDE.md lists `init`.

## Out of scope (deferred)

Per spec §12:

- `rupu init --interactive` (prompt for default provider/model).
- Template versioning / `rupu update-samples`.
- Project-name template substitution.
- Generic project files (README.md, LICENSE) — `--git` is the only "outside `.rupu/`" affordance.
- Sample updates over time pulled from a remote source.

## Release vehicle

**v0.3.0** — Slice B-2 shipped under v0.2.0. Tag after this branch merges per `docs/RELEASING.md`.
