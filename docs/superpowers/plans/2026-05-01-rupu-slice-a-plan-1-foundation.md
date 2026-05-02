# rupu Slice A — Plan 1: Foundation libraries

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the foundation Rust libraries for rupu — a Cargo workspace containing six independently-tested crates: `rupu-transcript`, `rupu-config`, `rupu-workspace`, `rupu-auth`, `rupu-providers` (lifted from phi-cell), and `rupu-tools`. Output is a workspace where `cargo test --workspace` passes; the runtime/CLI is built on top in Plan 2.

**Architecture:** Hexagonal — each crate defines its own domain types and trait boundaries; nothing in this plan instantiates concrete provider/auth backends in the agent loop (that's Plan 2). Tests use real I/O against tempdirs; no mocks except at the auth-keychain boundary where unit-testing the OS keychain is impractical.

**Tech Stack:** Rust stable (1.75+ MSRV), `tokio`, `serde`+`serde_json`, `toml`, `thiserror`, `tracing`, `ulid`, `keyring`, `tempfile`, `assert_fs`, `predicates`. Lifted phi-providers brings: `reqwest`, `async-trait`, `chrono`, `futures-util`, `ed25519-dalek`, `base64`, `fs2`.

**Spec:** `docs/superpowers/specs/2026-05-01-rupu-slice-a-design.md`

---

## File Structure

```
rupu/
  Cargo.toml                            # workspace root: members + workspace deps + lints
  rust-toolchain.toml                   # MSRV pin
  .gitignore
  CLAUDE.md                             # project memory for agents working on rupu
  .github/workflows/ci.yml              # PR-level CI: fmt + clippy + test
  crates/
    rupu-transcript/
      Cargo.toml
      src/
        lib.rs                          # re-exports
        event.rs                        # Event enum + variants + serde
        writer.rs                       # JsonlWriter (append-only)
        reader.rs                       # JsonlReader (handles aborted runs)
      tests/
        roundtrip.rs                    # event ser/de round-trip
        aborted.rs                      # readers handle missing run_complete
    rupu-config/
      Cargo.toml
      src/
        lib.rs
        config.rs                       # Config struct + serde
        layer.rs                        # global+project deep merge with array-replace
      tests/
        layering.rs
    rupu-workspace/
      Cargo.toml
      src/
        lib.rs
        record.rs                       # Workspace struct + TOML
        discover.rs                     # walk-up + canonicalize $PWD
        store.rs                        # upsert ~/.rupu/workspaces/<id>.toml
      tests/
        discover.rs
        upsert.rs
    rupu-auth/
      Cargo.toml
      src/
        lib.rs
        backend.rs                      # AuthBackend trait + ProviderId enum
        keyring.rs                      # KeyringBackend (uses `keyring` crate)
        json_file.rs                    # JsonFileBackend (chmod-600 fallback)
        probe.rs                        # backend selection + cache
      tests/
        json_file.rs
        probe_cache.rs
    rupu-providers/                     # LIFTED from phi-cell origin commit 3c7394cb...
      Cargo.toml
      src/                              # copied wholesale from phi-cell
      tests/                            # copied wholesale
    rupu-tools/
      Cargo.toml
      src/
        lib.rs
        tool.rs                         # Tool trait + ToolInput/ToolResult
        permission.rs                   # PermissionMode + PermissionGate
        bash.rs
        read_file.rs
        write_file.rs
        edit_file.rs
        grep.rs
        glob.rs
      tests/
        permission.rs
        bash.rs
        read_file.rs
        write_file.rs
        edit_file.rs
        grep.rs
        glob.rs
```

**Decomposition rationale:**
- Each crate has one responsibility and depends only on the crates listed in its `Cargo.toml` — `rupu-tools` depends on `rupu-transcript` (for derived events) but nothing else; `rupu-workspace` depends only on the standard library + `serde`/`toml`/`ulid`. The agent loop in Plan 2 wires these together.
- Per-crate test files in `tests/` are integration tests (black-box). Inline `#[cfg(test)] mod tests` lives next to the unit it tests.
- File splits are by responsibility (event types vs writer vs reader), not by technical layer. Files stay small enough to hold in context (~200 lines max).

**Micro-decisions made in this plan that the spec didn't specify** (consistent with the spec, called out so they're not invisible):

- MSRV pinned to **Rust 1.75** — recent enough for `let-else`, async-traits, conservative enough that most CI hosts have it.
- Workflow template engine: **`minijinja`** (locked during spec self-review).
- Default `maxTurns` for an agent file with no value set: **50**.
- Logger: **`tracing` + `tracing-subscriber`** (matches phi-cell).
- Transcript file naming: `<run_id>.jsonl` where `run_id` is a ULID.
- Workspace ID format: `ws_<26-char-ULID>`.
- Run ID format: `run_<26-char-ULID>`.
- Default agents shipped (referenced for Plan 2): `fix-bug`, `add-tests`, `review-diff`, `scaffold`, `summarize-diff`.

---

## Phase 0 — Workspace bootstrap

### Task 1: Initialize Cargo workspace

**Files:**
- Create: `Cargo.toml`
- Create: `rust-toolchain.toml`
- Create: `.gitignore`
- Create: `CLAUDE.md`

- [ ] **Step 1: Create the workspace root `Cargo.toml`**

```toml
[workspace]
resolver = "2"
members = [
    "crates/rupu-transcript",
    "crates/rupu-config",
    "crates/rupu-workspace",
    "crates/rupu-auth",
    "crates/rupu-providers",
    "crates/rupu-tools",
]

[workspace.package]
version = "0.1.0"
edition = "2021"
license = "Apache-2.0"
repository = "https://github.com/section9labs/rupu"
rust-version = "1.75"

[workspace.dependencies]
# Async runtime
tokio = { version = "1", features = ["full"] }

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
serde_yaml = "0.9"

# Errors
thiserror = "2"
anyhow = "1"

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter", "json"] }

# IDs / time
ulid = { version = "1", features = ["serde"] }
chrono = { version = "0.4", features = ["serde"] }

# HTTP / streaming (lifted phi-providers)
reqwest = { version = "0.12", features = ["json", "stream", "rustls-tls"], default-features = false }
async-trait = "0.1"
futures-util = "0.3"
fs2 = "0.4"

# Crypto (lifted)
ed25519-dalek = { version = "2", features = ["serde", "rand_core", "zeroize"] }
base64 = "0.22"

# CLI / templating (used in Plan 2 but pinned now for cohesion)
clap = { version = "4", features = ["derive"] }
minijinja = "2"

# Auth
keyring = "3"

# Test deps
tempfile = "3"
assert_fs = "1"
predicates = "3"

[workspace.lints.rust]
unsafe_code = "forbid"

[workspace.lints.clippy]
all = { level = "deny", priority = -1 }
```

- [ ] **Step 2: Create `rust-toolchain.toml`**

```toml
[toolchain]
channel = "1.75"
components = ["rustfmt", "clippy"]
```

- [ ] **Step 3: Create `.gitignore`**

```
/target
**/*.rs.bk
.DS_Store
.idea/
.vscode/
*.swp
```

- [ ] **Step 4: Create `CLAUDE.md`**

```markdown
# rupu — agentic code-development CLI

## Read first
- Slice A spec: `docs/superpowers/specs/2026-05-01-rupu-slice-a-design.md`
- Plan 1 (foundation libraries, in progress): `docs/superpowers/plans/2026-05-01-rupu-slice-a-plan-1-foundation.md`

## Architecture rules (enforced)
1. **Hexagonal separation.** `rupu-providers`, `rupu-tools`, `rupu-auth` define traits (ports). The agent runtime in `rupu-agent` (Plan 2) only knows traits.
2. **`rupu-cli` is thin** (Plan 2). Subcommands are arg parsing + delegation. No business logic in the CLI crate.
3. **Workspace deps only.** Versions pinned in root `Cargo.toml`; never in crate `Cargo.toml` files.
4. `#![deny(clippy::all)]` workspace-wide via `[workspace.lints]`. `unsafe_code` forbidden.

## Code standards
- Rust 2021, MSRV pinned in `rust-toolchain.toml`.
- Errors: `thiserror` for libraries; `anyhow` for the CLI binary (Plan 2).
- Async: `tokio`.
- Logging: `tracing` + `tracing-subscriber`.

## Heritage
- **Okesu** (`/Users/matt/Code/Oracle/Okesu`) — Go security-ops sibling. Same architectural shape (agent files = `.md` + YAML, JSONL transcripts, action protocol).
- **phi-cell** (`/Users/matt/Code/phi-cell`) — Rust workspace; `crates/phi-providers` is lifted near-verbatim into `crates/rupu-providers`. Lift origin: `Section9Labs/phi-cell` commit `3c7394cb1f5a87088954a1ff64fce86303066f55`.
```

- [ ] **Step 5: Verify workspace metadata loads**

Run: `cargo metadata --no-deps --format-version 1 > /dev/null`
Expected: exits 0 (workspace declares members but they don't exist yet — cargo will warn; that's fine for now). If cargo errors on missing crates, comment them out of `members` until later tasks create them.

Actually, do this instead — temporarily reduce `members` to an empty list for now:

Modify `Cargo.toml` `members` to `members = []` for this step. We'll add each member back as that crate is created.

Run: `cargo metadata --no-deps --format-version 1 > /dev/null`
Expected: exits 0.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml rust-toolchain.toml .gitignore CLAUDE.md
git commit -m "chore: initialize Cargo workspace skeleton"
```

---

### Task 2: PR-level CI workflow

**Files:**
- Create: `.github/workflows/ci.yml`

- [ ] **Step 1: Create the CI workflow**

```yaml
name: ci
on:
  pull_request:
  push:
    branches: [main]

jobs:
  fmt:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.75
        with:
          components: rustfmt
      - run: cargo fmt --all -- --check

  clippy:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.75
        with:
          components: clippy
      - uses: Swatinem/rust-cache@v2
      - run: cargo clippy --workspace --all-targets -- -D warnings

  test:
    strategy:
      matrix:
        os: [ubuntu-latest, macos-14]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.75
      - uses: Swatinem/rust-cache@v2
      - run: cargo test --workspace --all-targets

  release-build:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@1.75
      - uses: Swatinem/rust-cache@v2
      - run: cargo build --release --workspace
```

- [ ] **Step 2: Verify YAML is well-formed**

Run: `python3 -c 'import yaml,sys; yaml.safe_load(open(".github/workflows/ci.yml"))'`
Expected: exits 0 with no output.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci: add fmt + clippy + test + release-build workflow"
```

---

## Phase 1 — Foundation crates

### Task 3: `rupu-transcript` — crate skeleton

**Files:**
- Create: `crates/rupu-transcript/Cargo.toml`
- Create: `crates/rupu-transcript/src/lib.rs`
- Modify: `Cargo.toml` (add `crates/rupu-transcript` to members)

- [ ] **Step 1: Create the crate Cargo.toml**

```toml
[package]
name = "rupu-transcript"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true

[lints]
workspace = true

[dependencies]
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
chrono.workspace = true
ulid.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

- [ ] **Step 2: Create the empty `src/lib.rs`**

```rust
//! rupu transcript — JSONL event schema, writer, and reader.
//!
//! See `docs/transcript-schema.md` and the Slice A spec for the event
//! schema definition.

pub mod event;
pub mod reader;
pub mod writer;

pub use event::{Event, RunStatus};
pub use reader::{JsonlReader, ReadError, RunSummary};
pub use writer::{JsonlWriter, WriteError};
```

- [ ] **Step 3: Add the crate to the workspace members**

Modify `Cargo.toml` so `members` becomes:

```toml
members = [
    "crates/rupu-transcript",
]
```

- [ ] **Step 4: Verify it builds (will fail because event/reader/writer don't exist)**

Run: `cargo build -p rupu-transcript`
Expected: FAIL with "unresolved module" errors for event/reader/writer. Good — confirms the crate is wired in.

- [ ] **Step 5: Stub the modules so the crate compiles**

Create `crates/rupu-transcript/src/event.rs`:

```rust
//! Event schema for rupu transcripts. See `docs/transcript-schema.md`.
```

Create `crates/rupu-transcript/src/writer.rs`:

```rust
//! JSONL append-only writer.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum WriteError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialize: {0}")]
    Serialize(#[from] serde_json::Error),
}

pub struct JsonlWriter;
```

Create `crates/rupu-transcript/src/reader.rs`:

```rust
//! JSONL reader; handles missing `run_complete` (aborted runs).

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ReadError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse: {0}")]
    Parse(#[from] serde_json::Error),
}

pub struct JsonlReader;
pub struct RunSummary;
```

Adjust `lib.rs` re-exports as needed (drop `RunStatus` and `Event` for now since they don't exist):

```rust
pub mod event;
pub mod reader;
pub mod writer;

pub use reader::{JsonlReader, ReadError, RunSummary};
pub use writer::{JsonlWriter, WriteError};
```

Run: `cargo build -p rupu-transcript`
Expected: PASS (warnings about unused imports are OK).

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/rupu-transcript
git commit -m "feat(transcript): add crate skeleton"
```

---

### Task 4: `rupu-transcript` — Event types (TDD)

**Files:**
- Modify: `crates/rupu-transcript/src/event.rs`
- Modify: `crates/rupu-transcript/src/lib.rs` (re-add `Event`, `RunStatus` exports)
- Test: `crates/rupu-transcript/tests/roundtrip.rs`

- [ ] **Step 1: Write the failing round-trip test**

Create `crates/rupu-transcript/tests/roundtrip.rs`:

```rust
use rupu_transcript::event::{Event, RunStatus};

fn assert_roundtrip(e: &Event) {
    let json = serde_json::to_string(e).expect("serialize");
    let back: Event = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(e, &back, "roundtrip differed:\n  in:  {e:?}\n  out: {back:?}");
}

#[test]
fn roundtrip_run_start() {
    assert_roundtrip(&Event::RunStart {
        run_id: "run_01HXXX".into(),
        workspace_id: "ws_01HXXX".into(),
        agent: "fix-bug".into(),
        provider: "anthropic".into(),
        model: "claude-sonnet-4-6".into(),
        started_at: "2026-05-01T17:00:00Z".into(),
        mode: "ask".into(),
    });
}

#[test]
fn roundtrip_turn_start() {
    assert_roundtrip(&Event::TurnStart { turn_idx: 0 });
}

#[test]
fn roundtrip_assistant_message() {
    assert_roundtrip(&Event::AssistantMessage {
        content: "Looking at the failing test now.".into(),
        thinking: None,
    });
    assert_roundtrip(&Event::AssistantMessage {
        content: "Here's my plan.".into(),
        thinking: Some("First I'll grep for the symbol...".into()),
    });
}

#[test]
fn roundtrip_tool_call_and_result() {
    assert_roundtrip(&Event::ToolCall {
        call_id: "call_1".into(),
        tool: "bash".into(),
        input: serde_json::json!({ "command": "cargo test" }),
    });
    assert_roundtrip(&Event::ToolResult {
        call_id: "call_1".into(),
        output: "test result: ok. 12 passed".into(),
        error: None,
        duration_ms: 421,
    });
    assert_roundtrip(&Event::ToolResult {
        call_id: "call_2".into(),
        output: String::new(),
        error: Some("permission_denied".into()),
        duration_ms: 0,
    });
}

#[test]
fn roundtrip_file_edit() {
    assert_roundtrip(&Event::FileEdit {
        path: "src/lib.rs".into(),
        kind: "modify".into(),
        diff: "@@ -1,3 +1,4 @@\n fn foo() {\n+    todo!()\n }".into(),
    });
}

#[test]
fn roundtrip_command_run() {
    assert_roundtrip(&Event::CommandRun {
        argv: vec!["cargo".into(), "test".into()],
        cwd: "/Users/matt/Code/Oracle/rupu".into(),
        exit_code: 0,
        stdout_bytes: 4096,
        stderr_bytes: 128,
    });
}

#[test]
fn roundtrip_action_emitted() {
    assert_roundtrip(&Event::ActionEmitted {
        kind: "open_pr".into(),
        payload: serde_json::json!({ "title": "fix bug", "branch": "fix/123" }),
        allowed: true,
        applied: false,
        reason: None,
    });
    assert_roundtrip(&Event::ActionEmitted {
        kind: "delete_branch".into(),
        payload: serde_json::json!({}),
        allowed: false,
        applied: false,
        reason: Some("not in step allowlist".into()),
    });
}

#[test]
fn roundtrip_gate_requested() {
    assert_roundtrip(&Event::GateRequested {
        gate_id: "gate_1".into(),
        prompt: "Approve PR open?".into(),
        decision: None,
        decided_by: None,
    });
}

#[test]
fn roundtrip_turn_end() {
    assert_roundtrip(&Event::TurnEnd {
        turn_idx: 0,
        tokens_in: 1234,
        tokens_out: 567,
    });
}

#[test]
fn roundtrip_run_complete_ok() {
    assert_roundtrip(&Event::RunComplete {
        run_id: "run_01HXXX".into(),
        status: RunStatus::Ok,
        total_tokens: 5000,
        duration_ms: 12345,
        error: None,
    });
}

#[test]
fn roundtrip_run_complete_error_with_reason() {
    assert_roundtrip(&Event::RunComplete {
        run_id: "run_01HXXX".into(),
        status: RunStatus::Error,
        total_tokens: 5000,
        duration_ms: 12345,
        error: Some("context_overflow".into()),
    });
}
```

- [ ] **Step 2: Run the test — expect FAIL**

Run: `cargo test -p rupu-transcript --test roundtrip`
Expected: FAIL with "unresolved import `rupu_transcript::event::Event`" or similar.

- [ ] **Step 3: Implement the Event enum**

Replace `crates/rupu-transcript/src/event.rs` with:

```rust
//! Event schema for rupu transcripts. See `docs/transcript-schema.md`.
//!
//! All events are tagged JSON objects with a `type` discriminator and a
//! `data` payload. JSONL on disk is one event per line.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum Event {
    RunStart {
        run_id: String,
        workspace_id: String,
        agent: String,
        provider: String,
        model: String,
        started_at: String,
        mode: String,
    },
    TurnStart {
        turn_idx: u32,
    },
    AssistantMessage {
        content: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        thinking: Option<String>,
    },
    ToolCall {
        call_id: String,
        tool: String,
        input: Value,
    },
    ToolResult {
        call_id: String,
        output: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        error: Option<String>,
        duration_ms: u64,
    },
    FileEdit {
        path: String,
        kind: String,
        diff: String,
    },
    CommandRun {
        argv: Vec<String>,
        cwd: String,
        exit_code: i32,
        stdout_bytes: u64,
        stderr_bytes: u64,
    },
    ActionEmitted {
        kind: String,
        payload: Value,
        allowed: bool,
        applied: bool,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        reason: Option<String>,
    },
    GateRequested {
        gate_id: String,
        prompt: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        decision: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        decided_by: Option<String>,
    },
    TurnEnd {
        turn_idx: u32,
        tokens_in: u64,
        tokens_out: u64,
    },
    RunComplete {
        run_id: String,
        status: RunStatus,
        total_tokens: u64,
        duration_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        error: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Ok,
    Error,
    Aborted,
}
```

Update `crates/rupu-transcript/src/lib.rs` to add the re-exports:

```rust
//! rupu transcript — JSONL event schema, writer, and reader.

pub mod event;
pub mod reader;
pub mod writer;

pub use event::{Event, RunStatus};
pub use reader::{JsonlReader, ReadError, RunSummary};
pub use writer::{JsonlWriter, WriteError};
```

- [ ] **Step 4: Run the test — expect PASS**

Run: `cargo test -p rupu-transcript --test roundtrip`
Expected: 11 passing tests.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-transcript
git commit -m "feat(transcript): add Event enum with JSON round-trip tests"
```

---

### Task 5: `rupu-transcript` — JSONL writer (TDD)

**Files:**
- Modify: `crates/rupu-transcript/src/writer.rs`
- Test: `crates/rupu-transcript/tests/writer.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-transcript/tests/writer.rs`:

```rust
use rupu_transcript::{Event, JsonlWriter, RunStatus};
use tempfile::NamedTempFile;

#[test]
fn writes_events_one_per_line() {
    let f = NamedTempFile::new().unwrap();
    let mut w = JsonlWriter::create(f.path()).unwrap();
    w.write(&Event::TurnStart { turn_idx: 0 }).unwrap();
    w.write(&Event::TurnEnd {
        turn_idx: 0,
        tokens_in: 10,
        tokens_out: 20,
    })
    .unwrap();
    w.flush().unwrap();

    let content = std::fs::read_to_string(f.path()).unwrap();
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines.len(), 2, "expected 2 lines, got: {content}");
    assert!(lines[0].contains("\"turn_start\""));
    assert!(lines[1].contains("\"turn_end\""));
}

#[test]
fn append_extends_existing_file() {
    let f = NamedTempFile::new().unwrap();
    {
        let mut w = JsonlWriter::create(f.path()).unwrap();
        w.write(&Event::TurnStart { turn_idx: 0 }).unwrap();
    }
    {
        let mut w = JsonlWriter::append(f.path()).unwrap();
        w.write(&Event::TurnEnd {
            turn_idx: 0,
            tokens_in: 1,
            tokens_out: 1,
        })
        .unwrap();
    }
    let content = std::fs::read_to_string(f.path()).unwrap();
    assert_eq!(content.lines().count(), 2);
}

#[test]
fn each_line_is_valid_json() {
    let f = NamedTempFile::new().unwrap();
    let mut w = JsonlWriter::create(f.path()).unwrap();
    w.write(&Event::RunComplete {
        run_id: "run_x".into(),
        status: RunStatus::Ok,
        total_tokens: 0,
        duration_ms: 0,
        error: None,
    })
    .unwrap();
    let content = std::fs::read_to_string(f.path()).unwrap();
    for (i, line) in content.lines().enumerate() {
        let _: serde_json::Value =
            serde_json::from_str(line).unwrap_or_else(|e| panic!("line {i} not JSON: {e}"));
    }
}
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p rupu-transcript --test writer`
Expected: FAIL — `JsonlWriter::create` and `JsonlWriter::append` don't exist.

- [ ] **Step 3: Implement the writer**

Replace `crates/rupu-transcript/src/writer.rs` with:

```rust
//! JSONL append-only writer for transcript events.

use crate::event::Event;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum WriteError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialize: {0}")]
    Serialize(#[from] serde_json::Error),
}

pub struct JsonlWriter {
    inner: BufWriter<File>,
}

impl JsonlWriter {
    /// Create or truncate the file at `path`.
    pub fn create(path: impl AsRef<Path>) -> Result<Self, WriteError> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        Ok(Self {
            inner: BufWriter::new(f),
        })
    }

    /// Open `path` for append (create if missing).
    pub fn append(path: impl AsRef<Path>) -> Result<Self, WriteError> {
        if let Some(parent) = path.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let f = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            inner: BufWriter::new(f),
        })
    }

    pub fn write(&mut self, event: &Event) -> Result<(), WriteError> {
        let line = serde_json::to_string(event)?;
        self.inner.write_all(line.as_bytes())?;
        self.inner.write_all(b"\n")?;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<(), WriteError> {
        self.inner.flush()?;
        Ok(())
    }
}

impl Drop for JsonlWriter {
    fn drop(&mut self) {
        let _ = self.inner.flush();
    }
}
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rupu-transcript --test writer`
Expected: 3 passing tests.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-transcript
git commit -m "feat(transcript): add JsonlWriter with create/append modes"
```

---

### Task 6: `rupu-transcript` — JSONL reader with aborted-run detection (TDD)

**Files:**
- Modify: `crates/rupu-transcript/src/reader.rs`
- Test: `crates/rupu-transcript/tests/reader.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-transcript/tests/reader.rs`:

```rust
use rupu_transcript::{Event, JsonlReader, JsonlWriter, RunStatus};
use std::io::Write;
use tempfile::NamedTempFile;

fn write_events(path: &std::path::Path, events: &[Event]) {
    let mut w = JsonlWriter::create(path).unwrap();
    for e in events {
        w.write(e).unwrap();
    }
    w.flush().unwrap();
}

#[test]
fn reads_complete_run_summary() {
    let f = NamedTempFile::new().unwrap();
    write_events(
        f.path(),
        &[
            Event::RunStart {
                run_id: "run_a".into(),
                workspace_id: "ws_a".into(),
                agent: "fix-bug".into(),
                provider: "anthropic".into(),
                model: "claude-sonnet-4-6".into(),
                started_at: "2026-05-01T17:00:00Z".into(),
                mode: "ask".into(),
            },
            Event::TurnStart { turn_idx: 0 },
            Event::TurnEnd { turn_idx: 0, tokens_in: 10, tokens_out: 20 },
            Event::RunComplete {
                run_id: "run_a".into(),
                status: RunStatus::Ok,
                total_tokens: 30,
                duration_ms: 1000,
                error: None,
            },
        ],
    );
    let summary = JsonlReader::summary(f.path()).unwrap();
    assert_eq!(summary.run_id, "run_a");
    assert_eq!(summary.status, RunStatus::Ok);
    assert_eq!(summary.total_tokens, 30);
}

#[test]
fn missing_run_complete_reports_aborted() {
    let f = NamedTempFile::new().unwrap();
    write_events(
        f.path(),
        &[
            Event::RunStart {
                run_id: "run_b".into(),
                workspace_id: "ws_a".into(),
                agent: "fix-bug".into(),
                provider: "anthropic".into(),
                model: "claude-sonnet-4-6".into(),
                started_at: "2026-05-01T17:00:00Z".into(),
                mode: "ask".into(),
            },
            Event::TurnStart { turn_idx: 0 },
            // no TurnEnd, no RunComplete
        ],
    );
    let summary = JsonlReader::summary(f.path()).unwrap();
    assert_eq!(summary.status, RunStatus::Aborted);
    assert_eq!(summary.run_id, "run_b");
}

#[test]
fn truncated_last_line_does_not_crash() {
    let f = NamedTempFile::new().unwrap();
    {
        let mut w = JsonlWriter::create(f.path()).unwrap();
        w.write(&Event::RunStart {
            run_id: "run_c".into(),
            workspace_id: "ws_a".into(),
            agent: "x".into(),
            provider: "anthropic".into(),
            model: "claude-sonnet-4-6".into(),
            started_at: "2026-05-01T17:00:00Z".into(),
            mode: "ask".into(),
        })
        .unwrap();
    }
    // Append a partial JSON line (no trailing newline, malformed)
    let mut handle = std::fs::OpenOptions::new()
        .append(true)
        .open(f.path())
        .unwrap();
    handle.write_all(b"{\"type\":\"turn_start\"").unwrap();

    let summary = JsonlReader::summary(f.path()).unwrap();
    // Should still report as aborted (no run_complete present), not error
    assert_eq!(summary.status, RunStatus::Aborted);
    assert_eq!(summary.run_id, "run_c");
}

#[test]
fn iter_yields_all_events_in_order() {
    let f = NamedTempFile::new().unwrap();
    write_events(
        f.path(),
        &[
            Event::TurnStart { turn_idx: 0 },
            Event::TurnEnd { turn_idx: 0, tokens_in: 1, tokens_out: 2 },
            Event::TurnStart { turn_idx: 1 },
        ],
    );
    let events: Vec<_> = JsonlReader::iter(f.path())
        .unwrap()
        .filter_map(|r| r.ok())
        .collect();
    assert_eq!(events.len(), 3);
    matches!(events[0], Event::TurnStart { turn_idx: 0 });
    matches!(events[2], Event::TurnStart { turn_idx: 1 });
}
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p rupu-transcript --test reader`
Expected: FAIL — `JsonlReader::summary`, `JsonlReader::iter`, `RunSummary` shape don't exist.

- [ ] **Step 3: Implement the reader**

Replace `crates/rupu-transcript/src/reader.rs` with:

```rust
//! JSONL reader for transcript events.
//!
//! Aborted runs (no `run_complete` event) are surfaced via [`RunSummary`].
//! Truncated last lines are silently skipped (a partial last line is the
//! signature of an aborted/crashed write, not corruption to surface).

use crate::event::{Event, RunStatus};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ReadError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse: {0}")]
    Parse(#[from] serde_json::Error),
    #[error("transcript has no run_start event")]
    MissingRunStart,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunSummary {
    pub run_id: String,
    pub workspace_id: String,
    pub agent: String,
    pub provider: String,
    pub model: String,
    pub started_at: String,
    pub mode: String,
    pub status: RunStatus,
    pub total_tokens: u64,
    pub duration_ms: u64,
    pub error: Option<String>,
}

pub struct JsonlReader;

impl JsonlReader {
    /// Build a summary for the run by reading `run_start` and the last
    /// `run_complete`. If `run_complete` is absent, status is `Aborted`.
    pub fn summary(path: impl AsRef<Path>) -> Result<RunSummary, ReadError> {
        let mut start: Option<Event> = None;
        let mut complete: Option<Event> = None;

        for ev in Self::iter(path)? {
            match ev {
                Ok(e @ Event::RunStart { .. }) => start = Some(e),
                Ok(e @ Event::RunComplete { .. }) => complete = Some(e),
                Ok(_) => {}
                // Bad lines silently ignored (truncated tail of aborted run).
                Err(_) => {}
            }
        }

        let Some(Event::RunStart {
            run_id,
            workspace_id,
            agent,
            provider,
            model,
            started_at,
            mode,
        }) = start
        else {
            return Err(ReadError::MissingRunStart);
        };

        let (status, total_tokens, duration_ms, error) = match complete {
            Some(Event::RunComplete {
                status,
                total_tokens,
                duration_ms,
                error,
                ..
            }) => (status, total_tokens, duration_ms, error),
            _ => (RunStatus::Aborted, 0, 0, None),
        };

        Ok(RunSummary {
            run_id,
            workspace_id,
            agent,
            provider,
            model,
            started_at,
            mode,
            status,
            total_tokens,
            duration_ms,
            error,
        })
    }

    /// Stream events line-by-line. Bad lines yield `Err(ReadError::Parse)`;
    /// the iterator continues to the next line.
    pub fn iter(
        path: impl AsRef<Path>,
    ) -> Result<impl Iterator<Item = Result<Event, ReadError>>, ReadError> {
        let f = File::open(path)?;
        let reader = BufReader::new(f);
        Ok(reader.lines().map(|line_res| {
            let line = line_res.map_err(ReadError::Io)?;
            if line.trim().is_empty() {
                // Empty line — yield a parse error; callers filter these out.
                return Err(ReadError::Parse(serde_json::from_str::<Event>("").unwrap_err()));
            }
            serde_json::from_str(&line).map_err(ReadError::Parse)
        }))
    }
}
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rupu-transcript --test reader`
Expected: 4 passing tests.

- [ ] **Step 5: Run all transcript tests**

Run: `cargo test -p rupu-transcript`
Expected: All passing (11 roundtrip + 3 writer + 4 reader = 18 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-transcript
git commit -m "feat(transcript): add JsonlReader with aborted-run detection"
```

---

### Task 7: `rupu-config` — crate skeleton + Config type (TDD)

**Files:**
- Create: `crates/rupu-config/Cargo.toml`
- Create: `crates/rupu-config/src/lib.rs`
- Create: `crates/rupu-config/src/config.rs`
- Modify: `Cargo.toml` (add `crates/rupu-config` to members)
- Test: `crates/rupu-config/tests/parse.rs`

- [ ] **Step 1: Create the crate `Cargo.toml`**

```toml
[package]
name = "rupu-config"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true

[lints]
workspace = true

[dependencies]
serde.workspace = true
toml.workspace = true
thiserror.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

- [ ] **Step 2: Add to workspace members**

Modify root `Cargo.toml`:

```toml
members = [
    "crates/rupu-transcript",
    "crates/rupu-config",
]
```

- [ ] **Step 3: Write the failing test**

Create `crates/rupu-config/tests/parse.rs`:

```rust
use rupu_config::Config;

#[test]
fn parses_minimal_config() {
    let toml = r#"
        default_provider = "anthropic"
        default_model = "claude-sonnet-4-6"
    "#;
    let cfg: Config = toml::from_str(toml).expect("parse");
    assert_eq!(cfg.default_provider.as_deref(), Some("anthropic"));
    assert_eq!(cfg.default_model.as_deref(), Some("claude-sonnet-4-6"));
    assert_eq!(cfg.permission_mode, None);
}

#[test]
fn parses_full_config() {
    let toml = r#"
        default_provider = "anthropic"
        default_model = "claude-sonnet-4-6"
        permission_mode = "ask"
        log_level = "info"

        [bash]
        timeout_secs = 60
        env_allowlist = ["MY_VAR", "AWS_PROFILE"]

        [retry]
        max_attempts = 3
        initial_delay_ms = 200
    "#;
    let cfg: Config = toml::from_str(toml).expect("parse");
    assert_eq!(cfg.permission_mode.as_deref(), Some("ask"));
    assert_eq!(cfg.log_level.as_deref(), Some("info"));
    assert_eq!(cfg.bash.timeout_secs, Some(60));
    assert_eq!(
        cfg.bash.env_allowlist,
        Some(vec!["MY_VAR".into(), "AWS_PROFILE".into()])
    );
    assert_eq!(cfg.retry.max_attempts, Some(3));
}

#[test]
fn empty_config_is_valid() {
    let cfg: Config = toml::from_str("").expect("parse");
    assert_eq!(cfg.default_provider, None);
}
```

- [ ] **Step 4: Run — expect FAIL**

Run: `cargo test -p rupu-config --test parse`
Expected: FAIL — `Config` doesn't exist.

- [ ] **Step 5: Implement `Config`**

Create `crates/rupu-config/src/config.rs`:

```rust
//! Configuration types. See `docs/spec.md` for semantics.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub default_provider: Option<String>,
    pub default_model: Option<String>,
    pub permission_mode: Option<String>,
    pub log_level: Option<String>,
    pub bash: BashConfig,
    pub retry: RetryConfig,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BashConfig {
    pub timeout_secs: Option<u64>,
    pub env_allowlist: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RetryConfig {
    pub max_attempts: Option<u32>,
    pub initial_delay_ms: Option<u64>,
}
```

Create `crates/rupu-config/src/lib.rs`:

```rust
//! rupu-config — TOML-backed configuration with global+project layering.

pub mod config;
pub mod layer;

pub use config::{BashConfig, Config, RetryConfig};
pub use layer::{layer_files, LayerError};
```

Create a stub `crates/rupu-config/src/layer.rs` so the lib compiles:

```rust
//! Global+project config layering.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum LayerError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse: {0}")]
    Parse(#[from] toml::de::Error),
}

pub fn layer_files(_global: Option<&std::path::Path>, _project: Option<&std::path::Path>) -> Result<crate::Config, LayerError> {
    unimplemented!()
}
```

- [ ] **Step 6: Run — expect PASS**

Run: `cargo test -p rupu-config --test parse`
Expected: 3 passing tests.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/rupu-config
git commit -m "feat(config): add Config types with TOML parse tests"
```

---

### Task 8: `rupu-config` — Layering with array-replace semantics (TDD)

**Files:**
- Modify: `crates/rupu-config/src/layer.rs`
- Test: `crates/rupu-config/tests/layering.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-config/tests/layering.rs`:

```rust
use rupu_config::layer_files;
use std::io::Write;
use tempfile::NamedTempFile;

fn tmp_with(content: &str) -> NamedTempFile {
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(content.as_bytes()).unwrap();
    f
}

#[test]
fn project_overrides_global_scalar() {
    let g = tmp_with(
        r#"
default_provider = "anthropic"
default_model = "claude-sonnet-4-6"
"#,
    );
    let p = tmp_with(
        r#"
default_model = "claude-opus-4-7"
"#,
    );
    let cfg = layer_files(Some(g.path()), Some(p.path())).unwrap();
    assert_eq!(cfg.default_provider.as_deref(), Some("anthropic"));
    assert_eq!(cfg.default_model.as_deref(), Some("claude-opus-4-7"));
}

#[test]
fn project_overrides_global_table() {
    let g = tmp_with(
        r#"
[bash]
timeout_secs = 120
env_allowlist = ["A", "B"]
"#,
    );
    let p = tmp_with(
        r#"
[bash]
timeout_secs = 30
"#,
    );
    let cfg = layer_files(Some(g.path()), Some(p.path())).unwrap();
    assert_eq!(cfg.bash.timeout_secs, Some(30));
    // env_allowlist preserved from global because project didn't set it
    assert_eq!(
        cfg.bash.env_allowlist,
        Some(vec!["A".into(), "B".into()])
    );
}

#[test]
fn project_array_replaces_global_array_not_concat() {
    let g = tmp_with(
        r#"
[bash]
env_allowlist = ["A", "B", "C"]
"#,
    );
    let p = tmp_with(
        r#"
[bash]
env_allowlist = ["X"]
"#,
    );
    let cfg = layer_files(Some(g.path()), Some(p.path())).unwrap();
    // Critical: arrays REPLACE, never concat — so user can subtract
    assert_eq!(cfg.bash.env_allowlist, Some(vec!["X".into()]));
}

#[test]
fn missing_files_yield_empty_config() {
    let cfg = layer_files(None, None).unwrap();
    assert_eq!(cfg.default_provider, None);
}

#[test]
fn only_global_works() {
    let g = tmp_with(r#"default_provider = "openai""#);
    let cfg = layer_files(Some(g.path()), None).unwrap();
    assert_eq!(cfg.default_provider.as_deref(), Some("openai"));
}
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p rupu-config --test layering`
Expected: FAIL — current `layer_files` is `unimplemented!()`.

- [ ] **Step 3: Implement layering**

Replace `crates/rupu-config/src/layer.rs` with:

```rust
//! Global+project config layering.
//!
//! Rules (locked by spec):
//! - Project overrides global key-by-key (deep merge for tables).
//! - Arrays REPLACE — never concatenate. This is what allows users to
//!   subtract entries by re-declaring the array in the project file.

use crate::Config;
use std::path::Path;
use thiserror::Error;
use toml::Value;

#[derive(Debug, Error)]
pub enum LayerError {
    #[error("io reading {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parse {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("layered config invalid: {0}")]
    Layered(toml::de::Error),
}

pub fn layer_files(
    global: Option<&Path>,
    project: Option<&Path>,
) -> Result<Config, LayerError> {
    let global_v = read_optional_toml(global)?;
    let project_v = read_optional_toml(project)?;

    let merged = match (global_v, project_v) {
        (Some(g), Some(p)) => deep_merge(g, p),
        (Some(g), None) => g,
        (None, Some(p)) => p,
        (None, None) => Value::Table(toml::value::Table::new()),
    };

    let cfg: Config = merged.try_into().map_err(LayerError::Layered)?;
    Ok(cfg)
}

fn read_optional_toml(path: Option<&Path>) -> Result<Option<Value>, LayerError> {
    let Some(path) = path else { return Ok(None) };
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path).map_err(|e| LayerError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    let v: Value = toml::from_str(&text).map_err(|e| LayerError::Parse {
        path: path.display().to_string(),
        source: e,
    })?;
    Ok(Some(v))
}

/// Merge `overlay` into `base`. Tables merge key-by-key; everything else
/// (including arrays) is replaced wholesale by `overlay`.
fn deep_merge(base: Value, overlay: Value) -> Value {
    match (base, overlay) {
        (Value::Table(mut b), Value::Table(o)) => {
            for (k, v_overlay) in o {
                let merged = match b.remove(&k) {
                    Some(v_base) => deep_merge(v_base, v_overlay),
                    None => v_overlay,
                };
                b.insert(k, merged);
            }
            Value::Table(b)
        }
        // Anything else: overlay replaces base. Includes arrays.
        (_, overlay) => overlay,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_merge_replaces_arrays() {
        let base = toml::toml! {
            [t]
            arr = [1, 2, 3]
        };
        let overlay = toml::toml! {
            [t]
            arr = [9]
        };
        let merged = deep_merge(Value::Table(base), Value::Table(overlay));
        let arr = merged.get("t").unwrap().get("arr").unwrap();
        assert_eq!(arr, &Value::Array(vec![Value::Integer(9)]));
    }
}
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rupu-config`
Expected: All passing (3 parse + 5 layering + 1 unit = 9 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-config
git commit -m "feat(config): layering with array-replace deep merge"
```

---

### Task 9: `rupu-workspace` — crate skeleton + Workspace record (TDD)

**Files:**
- Create: `crates/rupu-workspace/Cargo.toml`
- Create: `crates/rupu-workspace/src/lib.rs`
- Create: `crates/rupu-workspace/src/record.rs`
- Modify: `Cargo.toml` (members)
- Test: `crates/rupu-workspace/tests/record.rs`

- [ ] **Step 1: Create crate Cargo.toml**

```toml
[package]
name = "rupu-workspace"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true

[lints]
workspace = true

[dependencies]
serde.workspace = true
toml.workspace = true
thiserror.workspace = true
ulid.workspace = true
chrono.workspace = true

[dev-dependencies]
tempfile.workspace = true
assert_fs.workspace = true
```

- [ ] **Step 2: Add to workspace members**

```toml
members = [
    "crates/rupu-transcript",
    "crates/rupu-config",
    "crates/rupu-workspace",
]
```

- [ ] **Step 3: Write the failing test**

Create `crates/rupu-workspace/tests/record.rs`:

```rust
use rupu_workspace::Workspace;

#[test]
fn round_trip_workspace_toml() {
    let ws = Workspace {
        id: "ws_01HXXX0123456789ABCDEFGHJK".into(),
        path: "/Users/matt/Code/rupu".into(),
        repo_remote: Some("git@github.com:section9labs/rupu.git".into()),
        default_branch: Some("main".into()),
        created_at: "2026-05-01T17:00:00Z".into(),
        last_run_at: Some("2026-05-01T17:42:00Z".into()),
    };
    let serialized = toml::to_string(&ws).unwrap();
    let back: Workspace = toml::from_str(&serialized).unwrap();
    assert_eq!(ws, back);
}

#[test]
fn parses_minimal_workspace_toml() {
    let toml = r#"
id              = "ws_01HXXX0123456789ABCDEFGHJK"
path            = "/Users/matt/Code/rupu"
created_at      = "2026-05-01T17:00:00Z"
"#;
    let ws: Workspace = toml::from_str(toml).unwrap();
    assert_eq!(ws.repo_remote, None);
    assert_eq!(ws.default_branch, None);
}
```

- [ ] **Step 4: Run — expect FAIL**

Run: `cargo test -p rupu-workspace --test record`
Expected: FAIL — crate doesn't compile yet.

- [ ] **Step 5: Implement Workspace record**

Create `crates/rupu-workspace/src/record.rs`:

```rust
//! Workspace record. Stored at `~/.rupu/workspaces/<id>.toml`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub repo_remote: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub default_branch: Option<String>,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_run_at: Option<String>,
}

/// ULID-prefixed workspace id, e.g. `ws_01HXXX...`.
pub fn new_id() -> String {
    format!("ws_{}", ulid::Ulid::new())
}
```

Create `crates/rupu-workspace/src/lib.rs`:

```rust
//! rupu-workspace — workspace discovery and record store.

pub mod discover;
pub mod record;
pub mod store;

pub use discover::{discover, DiscoverError, Discovery};
pub use record::{new_id, Workspace};
pub use store::{upsert, StoreError, WorkspaceStore};
```

Stub the missing modules so the crate compiles:

`crates/rupu-workspace/src/discover.rs`:

```rust
use thiserror::Error;
use std::path::{Path, PathBuf};

#[derive(Debug, Error)]
pub enum DiscoverError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub struct Discovery {
    pub project_root: Option<PathBuf>,
    pub canonical_pwd: PathBuf,
}

pub fn discover(_pwd: &Path) -> Result<Discovery, DiscoverError> {
    unimplemented!()
}
```

`crates/rupu-workspace/src/store.rs`:

```rust
use crate::Workspace;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("serialize: {0}")]
    Ser(#[from] toml::ser::Error),
}

pub struct WorkspaceStore {
    pub root: PathBuf,
}

pub fn upsert(_store: &WorkspaceStore, _path: &Path) -> Result<Workspace, StoreError> {
    unimplemented!()
}
```

- [ ] **Step 6: Run — expect PASS**

Run: `cargo test -p rupu-workspace --test record`
Expected: 2 passing tests.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml crates/rupu-workspace
git commit -m "feat(workspace): add Workspace record + ULID id helper"
```

---

### Task 10: `rupu-workspace` — discover() walk-up + canonicalize (TDD)

**Files:**
- Modify: `crates/rupu-workspace/src/discover.rs`
- Test: `crates/rupu-workspace/tests/discover.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-workspace/tests/discover.rs`:

```rust
use assert_fs::prelude::*;
use rupu_workspace::discover;

#[test]
fn finds_rupu_dir_in_pwd() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child(".rupu").create_dir_all().unwrap();
    let d = discover(tmp.path()).unwrap();
    assert_eq!(
        d.project_root.as_deref().map(|p| p.canonicalize().unwrap()),
        Some(tmp.path().canonicalize().unwrap())
    );
    assert_eq!(d.canonical_pwd, tmp.path().canonicalize().unwrap());
}

#[test]
fn walks_up_to_find_rupu_dir() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child(".rupu").create_dir_all().unwrap();
    let nested = tmp.child("a/b/c");
    nested.create_dir_all().unwrap();

    let d = discover(nested.path()).unwrap();
    assert_eq!(
        d.project_root.as_deref().map(|p| p.canonicalize().unwrap()),
        Some(tmp.path().canonicalize().unwrap())
    );
}

#[test]
fn no_rupu_dir_means_no_project_root() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let nested = tmp.child("x/y");
    nested.create_dir_all().unwrap();
    let d = discover(nested.path()).unwrap();
    assert!(d.project_root.is_none());
    assert_eq!(d.canonical_pwd, nested.path().canonicalize().unwrap());
}
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p rupu-workspace --test discover`
Expected: FAIL — `discover` is `unimplemented!()`.

- [ ] **Step 3: Implement discover**

Replace `crates/rupu-workspace/src/discover.rs` with:

```rust
//! Project discovery: walk up from `$PWD` looking for the first `.rupu/`
//! directory (mirrors how `git` finds `.git`).

use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum DiscoverError {
    #[error("io canonicalizing {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
}

#[derive(Debug, Clone)]
pub struct Discovery {
    pub project_root: Option<PathBuf>,
    pub canonical_pwd: PathBuf,
}

pub fn discover(pwd: &Path) -> Result<Discovery, DiscoverError> {
    let canonical_pwd = pwd.canonicalize().map_err(|e| DiscoverError::Io {
        path: pwd.display().to_string(),
        source: e,
    })?;

    let mut cursor: Option<&Path> = Some(&canonical_pwd);
    while let Some(dir) = cursor {
        if dir.join(".rupu").is_dir() {
            return Ok(Discovery {
                project_root: Some(dir.to_path_buf()),
                canonical_pwd: canonical_pwd.clone(),
            });
        }
        cursor = dir.parent();
    }

    Ok(Discovery {
        project_root: None,
        canonical_pwd,
    })
}
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rupu-workspace --test discover`
Expected: 3 passing tests.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-workspace
git commit -m "feat(workspace): add discover() with walk-up + canonicalize"
```

---

### Task 11: `rupu-workspace` — upsert() (TDD)

**Files:**
- Modify: `crates/rupu-workspace/src/store.rs`
- Test: `crates/rupu-workspace/tests/upsert.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-workspace/tests/upsert.rs`:

```rust
use assert_fs::prelude::*;
use rupu_workspace::{upsert, WorkspaceStore};

#[test]
fn first_upsert_creates_record_with_new_id() {
    let store_dir = assert_fs::TempDir::new().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    let store = WorkspaceStore { root: store_dir.path().to_path_buf() };

    let ws = upsert(&store, project.path()).unwrap();
    assert!(ws.id.starts_with("ws_"));
    assert_eq!(
        std::path::Path::new(&ws.path).canonicalize().unwrap(),
        project.path().canonicalize().unwrap()
    );

    // The record file exists at <store_dir>/<id>.toml
    let recorded = store_dir.child(format!("{}.toml", ws.id));
    recorded.assert(predicates::path::is_file());
}

#[test]
fn second_upsert_in_same_path_returns_same_id() {
    let store_dir = assert_fs::TempDir::new().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    let store = WorkspaceStore { root: store_dir.path().to_path_buf() };

    let ws1 = upsert(&store, project.path()).unwrap();
    let ws2 = upsert(&store, project.path()).unwrap();
    assert_eq!(ws1.id, ws2.id);
}

#[test]
fn second_upsert_updates_last_run_at() {
    let store_dir = assert_fs::TempDir::new().unwrap();
    let project = assert_fs::TempDir::new().unwrap();
    let store = WorkspaceStore { root: store_dir.path().to_path_buf() };

    let ws1 = upsert(&store, project.path()).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let ws2 = upsert(&store, project.path()).unwrap();
    assert_ne!(ws1.last_run_at, ws2.last_run_at, "last_run_at should advance");
}
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p rupu-workspace --test upsert`
Expected: FAIL — `upsert` is `unimplemented!()`.

- [ ] **Step 3: Implement upsert**

Replace `crates/rupu-workspace/src/store.rs` with:

```rust
//! Workspace record store. Lives at `~/.rupu/workspaces/`.
//!
//! Records are keyed by canonicalized path; on `upsert` we read every
//! record in the store dir and reuse the matching one rather than
//! generating a new id.

use crate::record::{new_id, Workspace};
use chrono::Utc;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("io {action}: {source}")]
    Io {
        action: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parse {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("serialize: {0}")]
    Ser(#[from] toml::ser::Error),
}

#[derive(Debug, Clone)]
pub struct WorkspaceStore {
    pub root: PathBuf,
}

impl WorkspaceStore {
    fn ensure_root(&self) -> Result<(), StoreError> {
        std::fs::create_dir_all(&self.root).map_err(|e| StoreError::Io {
            action: format!("create_dir_all {}", self.root.display()),
            source: e,
        })
    }

    fn record_path(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.toml"))
    }

    fn list(&self) -> Result<Vec<Workspace>, StoreError> {
        if !self.root.exists() {
            return Ok(vec![]);
        }
        let mut out = vec![];
        for entry in std::fs::read_dir(&self.root).map_err(|e| StoreError::Io {
            action: format!("read_dir {}", self.root.display()),
            source: e,
        })? {
            let entry = entry.map_err(|e| StoreError::Io {
                action: "read_dir entry".into(),
                source: e,
            })?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("toml") {
                continue;
            }
            let text = std::fs::read_to_string(&path).map_err(|e| StoreError::Io {
                action: format!("read {}", path.display()),
                source: e,
            })?;
            let ws: Workspace =
                toml::from_str(&text).map_err(|e| StoreError::Parse {
                    path: path.display().to_string(),
                    source: e,
                })?;
            out.push(ws);
        }
        Ok(out)
    }

    fn write(&self, ws: &Workspace) -> Result<(), StoreError> {
        self.ensure_root()?;
        let body = toml::to_string(ws)?;
        let path = self.record_path(&ws.id);
        std::fs::write(&path, body).map_err(|e| StoreError::Io {
            action: format!("write {}", path.display()),
            source: e,
        })
    }
}

/// Look up an existing workspace for `path` (canonicalized) or create a
/// new one. Bumps `last_run_at` to "now" in either case.
pub fn upsert(store: &WorkspaceStore, path: &Path) -> Result<Workspace, StoreError> {
    let canonical = path.canonicalize().map_err(|e| StoreError::Io {
        action: format!("canonicalize {}", path.display()),
        source: e,
    })?;
    let canonical_str = canonical.display().to_string();

    let now = Utc::now().to_rfc3339();
    let existing = store.list()?.into_iter().find(|w| {
        Path::new(&w.path)
            .canonicalize()
            .map(|p| p == canonical)
            .unwrap_or(false)
    });

    let ws = match existing {
        Some(mut w) => {
            w.last_run_at = Some(now);
            w
        }
        None => Workspace {
            id: new_id(),
            path: canonical_str,
            repo_remote: detect_repo_remote(&canonical),
            default_branch: detect_default_branch(&canonical),
            created_at: now.clone(),
            last_run_at: Some(now),
        },
    };

    store.write(&ws)?;
    Ok(ws)
}

fn detect_repo_remote(path: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["-C"])
        .arg(path)
        .args(["remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn detect_default_branch(path: &Path) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["-C"])
        .arg(path)
        .args(["symbolic-ref", "--short", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}
```

- [ ] **Step 2 (verify): Run — expect PASS**

Run: `cargo test -p rupu-workspace`
Expected: 2 record + 3 discover + 3 upsert = 8 passing tests.

- [ ] **Step 3: Commit**

```bash
git add crates/rupu-workspace
git commit -m "feat(workspace): upsert() with path-keyed reuse"
```

---

### Task 12: `rupu-auth` — crate skeleton + AuthBackend trait

**Files:**
- Create: `crates/rupu-auth/Cargo.toml`
- Create: `crates/rupu-auth/src/lib.rs`
- Create: `crates/rupu-auth/src/backend.rs`
- Modify: `Cargo.toml` (members)

- [ ] **Step 1: Create crate Cargo.toml**

```toml
[package]
name = "rupu-auth"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true

[lints]
workspace = true

[dependencies]
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tracing.workspace = true
keyring.workspace = true

[dev-dependencies]
tempfile.workspace = true
assert_fs.workspace = true
```

- [ ] **Step 2: Add to workspace members**

```toml
members = [
    "crates/rupu-transcript",
    "crates/rupu-config",
    "crates/rupu-workspace",
    "crates/rupu-auth",
]
```

- [ ] **Step 3: Create the trait + provider id**

Create `crates/rupu-auth/src/backend.rs`:

```rust
//! Auth backend trait. Implementations: keyring, JSON file (chmod 600).

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderId {
    Anthropic,
    Openai,
    Copilot,
    Local,
}

impl ProviderId {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic",
            Self::Openai => "openai",
            Self::Copilot => "copilot",
            Self::Local => "local",
        }
    }
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialize: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("keyring: {0}")]
    Keyring(String),
    #[error("not configured for provider {0}")]
    NotConfigured(&'static str),
}

pub trait AuthBackend: Send + Sync {
    /// Store `secret` for `provider`.
    fn store(&self, provider: ProviderId, secret: &str) -> Result<(), AuthError>;
    /// Retrieve the secret for `provider`. Returns `NotConfigured` if absent.
    fn retrieve(&self, provider: ProviderId) -> Result<String, AuthError>;
    /// Forget the secret for `provider`. No-op if absent.
    fn forget(&self, provider: ProviderId) -> Result<(), AuthError>;
    /// Human-readable backend name (for `rupu auth status`).
    fn name(&self) -> &'static str;
}
```

Create `crates/rupu-auth/src/lib.rs`:

```rust
//! rupu-auth — credential storage with OS keychain + chmod-600 fallback.

pub mod backend;
pub mod json_file;
pub mod keyring;
pub mod probe;

pub use backend::{AuthBackend, AuthError, ProviderId};
pub use json_file::JsonFileBackend;
pub use keyring::KeyringBackend;
pub use probe::{select_backend, BackendChoice, ProbeCache};
```

Stub `crates/rupu-auth/src/keyring.rs`:

```rust
use crate::backend::{AuthBackend, AuthError, ProviderId};

pub struct KeyringBackend;

impl KeyringBackend {
    pub fn new() -> Self { Self }
    /// Returns Ok(()) if the keychain is reachable; Err otherwise.
    pub fn probe() -> Result<(), AuthError> { unimplemented!() }
}

impl AuthBackend for KeyringBackend {
    fn store(&self, _p: ProviderId, _s: &str) -> Result<(), AuthError> { unimplemented!() }
    fn retrieve(&self, _p: ProviderId) -> Result<String, AuthError> { unimplemented!() }
    fn forget(&self, _p: ProviderId) -> Result<(), AuthError> { unimplemented!() }
    fn name(&self) -> &'static str { "os-keychain" }
}
```

Stub `crates/rupu-auth/src/json_file.rs`:

```rust
use crate::backend::{AuthBackend, AuthError, ProviderId};
use std::path::PathBuf;

pub struct JsonFileBackend {
    pub path: PathBuf,
}

impl AuthBackend for JsonFileBackend {
    fn store(&self, _p: ProviderId, _s: &str) -> Result<(), AuthError> { unimplemented!() }
    fn retrieve(&self, _p: ProviderId) -> Result<String, AuthError> { unimplemented!() }
    fn forget(&self, _p: ProviderId) -> Result<(), AuthError> { unimplemented!() }
    fn name(&self) -> &'static str { "json-file" }
}
```

Stub `crates/rupu-auth/src/probe.rs`:

```rust
use crate::backend::AuthBackend;
use std::path::PathBuf;

pub enum BackendChoice {
    Keyring,
    JsonFile,
}

pub struct ProbeCache {
    pub path: PathBuf,
}

pub fn select_backend(_cache: &ProbeCache, _fallback_path: PathBuf) -> Box<dyn AuthBackend> {
    unimplemented!()
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build -p rupu-auth`
Expected: PASS (warnings about unused params are OK).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/rupu-auth
git commit -m "feat(auth): add AuthBackend trait + provider ids"
```

---

### Task 13: `rupu-auth` — JsonFileBackend with chmod-600 enforcement (TDD)

**Files:**
- Modify: `crates/rupu-auth/src/json_file.rs`
- Test: `crates/rupu-auth/tests/json_file.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-auth/tests/json_file.rs`:

```rust
#![cfg(unix)]

use assert_fs::prelude::*;
use rupu_auth::{AuthBackend, JsonFileBackend, ProviderId};
use std::os::unix::fs::PermissionsExt;

#[test]
fn store_and_retrieve_round_trip() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let path = tmp.child("auth.json").to_path_buf();
    let b = JsonFileBackend { path: path.clone() };

    b.store(ProviderId::Anthropic, "sk-ant-XXX").unwrap();
    let got = b.retrieve(ProviderId::Anthropic).unwrap();
    assert_eq!(got, "sk-ant-XXX");
}

#[test]
fn store_creates_file_with_mode_0600() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let path = tmp.child("auth.json").to_path_buf();
    let b = JsonFileBackend { path: path.clone() };

    b.store(ProviderId::Anthropic, "k").unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "expected 0600, got {:o}", mode);
}

#[test]
fn forget_removes_only_target_provider() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let path = tmp.child("auth.json").to_path_buf();
    let b = JsonFileBackend { path: path.clone() };

    b.store(ProviderId::Anthropic, "a").unwrap();
    b.store(ProviderId::Openai, "o").unwrap();
    b.forget(ProviderId::Anthropic).unwrap();
    assert!(b.retrieve(ProviderId::Anthropic).is_err());
    assert_eq!(b.retrieve(ProviderId::Openai).unwrap(), "o");
}

#[test]
fn retrieve_missing_provider_returns_not_configured_error() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let path = tmp.child("auth.json").to_path_buf();
    let b = JsonFileBackend { path };
    let err = b.retrieve(ProviderId::Anthropic).unwrap_err();
    assert!(matches!(err, rupu_auth::AuthError::NotConfigured(_)));
}

#[test]
fn wrong_mode_emits_warning_but_still_reads() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = assert_fs::TempDir::new().unwrap();
    let path = tmp.child("auth.json").to_path_buf();
    let b = JsonFileBackend { path: path.clone() };
    b.store(ProviderId::Anthropic, "k").unwrap();

    // Make it world-readable
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o644);
    std::fs::set_permissions(&path, perms).unwrap();

    // Should still retrieve successfully (warn, not fail)
    let got = b.retrieve(ProviderId::Anthropic).unwrap();
    assert_eq!(got, "k");
}
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p rupu-auth --test json_file`
Expected: FAIL — `JsonFileBackend` methods are `unimplemented!()`.

- [ ] **Step 3: Implement**

Replace `crates/rupu-auth/src/json_file.rs` with:

```rust
//! Plaintext JSON file backend with chmod-600 enforcement.
//! Used when the OS keychain is unreachable.

use crate::backend::{AuthBackend, AuthError, ProviderId};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tracing::warn;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Stored {
    #[serde(default, flatten)]
    secrets: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct JsonFileBackend {
    pub path: PathBuf,
}

impl JsonFileBackend {
    fn read(&self) -> Result<Stored, AuthError> {
        if !self.path.exists() {
            return Ok(Stored::default());
        }
        self.warn_on_wrong_mode();
        let text = std::fs::read_to_string(&self.path)?;
        let s: Stored = serde_json::from_str(&text)?;
        Ok(s)
    }

    fn write(&self, s: &Stored) -> Result<(), AuthError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string_pretty(s)?;
        std::fs::write(&self.path, body)?;
        self.set_mode_0600();
        Ok(())
    }

    #[cfg(unix)]
    fn set_mode_0600(&self) {
        if let Ok(meta) = std::fs::metadata(&self.path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(&self.path, perms);
        }
    }

    #[cfg(not(unix))]
    fn set_mode_0600(&self) {}

    #[cfg(unix)]
    fn warn_on_wrong_mode(&self) {
        if let Ok(meta) = std::fs::metadata(&self.path) {
            let mode = meta.permissions().mode() & 0o777;
            if mode != 0o600 {
                warn!(
                    path = %self.path.display(),
                    mode = format!("{:o}", mode),
                    "auth.json should be mode 0600 — fix with: chmod 600 {}",
                    self.path.display()
                );
            }
        }
    }

    #[cfg(not(unix))]
    fn warn_on_wrong_mode(&self) {}
}

impl AuthBackend for JsonFileBackend {
    fn store(&self, p: ProviderId, secret: &str) -> Result<(), AuthError> {
        let mut s = self.read()?;
        s.secrets.insert(p.as_str().to_string(), secret.to_string());
        self.write(&s)
    }

    fn retrieve(&self, p: ProviderId) -> Result<String, AuthError> {
        let s = self.read()?;
        s.secrets
            .get(p.as_str())
            .cloned()
            .ok_or(AuthError::NotConfigured(provider_static_str(p)))
    }

    fn forget(&self, p: ProviderId) -> Result<(), AuthError> {
        let mut s = self.read()?;
        s.secrets.remove(p.as_str());
        self.write(&s)
    }

    fn name(&self) -> &'static str {
        "json-file"
    }
}

fn provider_static_str(p: ProviderId) -> &'static str {
    match p {
        ProviderId::Anthropic => "anthropic",
        ProviderId::Openai => "openai",
        ProviderId::Copilot => "copilot",
        ProviderId::Local => "local",
    }
}
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rupu-auth --test json_file`
Expected: 5 passing tests.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-auth
git commit -m "feat(auth): JsonFileBackend with chmod-600 enforcement"
```

---

### Task 14: `rupu-auth` — KeyringBackend (probe + delegating store/retrieve)

**Files:**
- Modify: `crates/rupu-auth/src/keyring.rs`
- Test: `crates/rupu-auth/tests/keyring_ignored.rs`

- [ ] **Step 1: Implement the keyring backend**

Replace `crates/rupu-auth/src/keyring.rs` with:

```rust
//! OS keychain backend (macOS Keychain / Linux Secret Service / Windows
//! Credential Manager via the `keyring` crate). Probe failure is what
//! triggers fallback to `JsonFileBackend`.

use crate::backend::{AuthBackend, AuthError, ProviderId};

const SERVICE: &str = "rupu";

#[derive(Debug, Default, Clone)]
pub struct KeyringBackend;

impl KeyringBackend {
    pub fn new() -> Self {
        Self
    }

    /// Probe for keychain availability by attempting a no-op set+delete on
    /// a sentinel entry. Returns `Ok(())` if the keychain is reachable.
    pub fn probe() -> Result<(), AuthError> {
        let entry = ::keyring::Entry::new(SERVICE, "__probe__")
            .map_err(|e| AuthError::Keyring(e.to_string()))?;
        // Try set then delete; either failing means we should fall back.
        entry
            .set_password("probe")
            .map_err(|e| AuthError::Keyring(e.to_string()))?;
        // Best-effort cleanup
        let _ = entry.delete_credential();
        Ok(())
    }

    fn entry(&self, p: ProviderId) -> Result<::keyring::Entry, AuthError> {
        ::keyring::Entry::new(SERVICE, p.as_str()).map_err(|e| AuthError::Keyring(e.to_string()))
    }
}

impl AuthBackend for KeyringBackend {
    fn store(&self, p: ProviderId, secret: &str) -> Result<(), AuthError> {
        self.entry(p)?
            .set_password(secret)
            .map_err(|e| AuthError::Keyring(e.to_string()))
    }

    fn retrieve(&self, p: ProviderId) -> Result<String, AuthError> {
        match self.entry(p)?.get_password() {
            Ok(s) => Ok(s),
            Err(::keyring::Error::NoEntry) => Err(AuthError::NotConfigured(p.as_str())),
            Err(e) => Err(AuthError::Keyring(e.to_string())),
        }
    }

    fn forget(&self, p: ProviderId) -> Result<(), AuthError> {
        match self.entry(p)?.delete_credential() {
            Ok(()) | Err(::keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(AuthError::Keyring(e.to_string())),
        }
    }

    fn name(&self) -> &'static str {
        "os-keychain"
    }
}
```

- [ ] **Step 2: Add `--ignored` smoke test**

Create `crates/rupu-auth/tests/keyring_ignored.rs`:

```rust
//! These tests touch the real OS keychain; run with:
//!   cargo test -p rupu-auth -- --ignored

use rupu_auth::{AuthBackend, KeyringBackend, ProviderId};

#[test]
#[ignore]
fn real_keyring_round_trip() {
    if KeyringBackend::probe().is_err() {
        eprintln!("skipping: keyring not available");
        return;
    }
    let b = KeyringBackend::new();
    b.store(ProviderId::Anthropic, "test-secret-zzz").unwrap();
    let got = b.retrieve(ProviderId::Anthropic).unwrap();
    assert_eq!(got, "test-secret-zzz");
    b.forget(ProviderId::Anthropic).unwrap();
}
```

- [ ] **Step 3: Verify it compiles (do not run --ignored)**

Run: `cargo build -p rupu-auth --tests`
Expected: PASS.

- [ ] **Step 4: Run non-ignored tests**

Run: `cargo test -p rupu-auth`
Expected: 5 json_file tests pass; the ignored keyring test is skipped.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-auth
git commit -m "feat(auth): KeyringBackend with probe + ignored real-keychain test"
```

---

### Task 15: `rupu-auth` — Probe + cache (TDD)

**Files:**
- Modify: `crates/rupu-auth/src/probe.rs`
- Test: `crates/rupu-auth/tests/probe_cache.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-auth/tests/probe_cache.rs`:

```rust
use assert_fs::prelude::*;
use rupu_auth::{ProbeCache};

#[test]
fn writes_cache_file_on_first_call() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let cache_path = tmp.child("cache.json").to_path_buf();
    let cache = ProbeCache { path: cache_path.clone() };

    let _backend = rupu_auth::select_backend(&cache, tmp.child("auth.json").to_path_buf());

    cache_path.assert(predicates::path::is_file());
}

#[test]
fn second_call_uses_cached_choice() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let cache_path = tmp.child("cache.json").to_path_buf();
    let cache = ProbeCache { path: cache_path.clone() };

    let b1 = rupu_auth::select_backend(&cache, tmp.child("auth.json").to_path_buf());
    let b2 = rupu_auth::select_backend(&cache, tmp.child("auth.json").to_path_buf());
    assert_eq!(b1.name(), b2.name());
}

#[test]
fn invalidate_clears_cache() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let cache_path = tmp.child("cache.json").to_path_buf();
    let cache = ProbeCache { path: cache_path.clone() };

    let _ = rupu_auth::select_backend(&cache, tmp.child("auth.json").to_path_buf());
    cache.invalidate().unwrap();
    cache_path.assert(predicates::path::missing());
}
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p rupu-auth --test probe_cache`
Expected: FAIL — `select_backend` is `unimplemented!()` and `ProbeCache` lacks `invalidate`.

- [ ] **Step 3: Implement probe + cache**

Replace `crates/rupu-auth/src/probe.rs` with:

```rust
//! Backend probe + cached choice. Avoids re-probing the keychain on
//! every CLI invocation.

use crate::backend::AuthBackend;
use crate::json_file::JsonFileBackend;
use crate::keyring::KeyringBackend;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::warn;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackendChoice {
    Keyring,
    JsonFile,
}

#[derive(Debug, Clone)]
pub struct ProbeCache {
    pub path: PathBuf,
}

impl ProbeCache {
    pub fn read(&self) -> Option<BackendChoice> {
        let text = std::fs::read_to_string(&self.path).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn write(&self, c: BackendChoice) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, serde_json::to_string(&c).unwrap())
    }

    pub fn invalidate(&self) -> std::io::Result<()> {
        match std::fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

pub fn select_backend(cache: &ProbeCache, fallback_path: PathBuf) -> Box<dyn AuthBackend> {
    let choice = cache.read().unwrap_or_else(|| {
        let chosen = match KeyringBackend::probe() {
            Ok(()) => BackendChoice::Keyring,
            Err(e) => {
                warn!(
                    error = %e,
                    "OS keychain unavailable; falling back to chmod-600 JSON file ({})",
                    fallback_path.display()
                );
                BackendChoice::JsonFile
            }
        };
        if let Err(e) = cache.write(chosen) {
            warn!(error = %e, "failed to write probe cache; will re-probe next run");
        }
        chosen
    });

    match choice {
        BackendChoice::Keyring => Box::new(KeyringBackend::new()),
        BackendChoice::JsonFile => Box::new(JsonFileBackend { path: fallback_path }),
    }
}
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rupu-auth`
Expected: 5 json_file + 3 probe_cache = 8 passing tests.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-auth
git commit -m "feat(auth): probe with cache + select_backend"
```

---

### Task 16: `rupu-providers` — Lift from phi-cell

**Goal:** Copy `phi-cell/crates/phi-providers` wholesale, adapt the package name and dependencies to use rupu's workspace, verify the lifted tests pass.

**Files:**
- Create: `crates/rupu-providers/` (copied from phi-cell)
- Modify: root `Cargo.toml` (add to members)

- [ ] **Step 1: Capture origin commit + branch**

Run:
```bash
cd /Users/matt/Code/phi-cell
git rev-parse origin/main
```
Expected: prints a commit hash. **Record this hash** — it goes into the commit message and `crates/rupu-providers/LIFT_ORIGIN.md`.

For this plan, the captured hash is `3c7394cb1f5a87088954a1ff64fce86303066f55` from `Section9Labs/phi-cell` `origin/main` on 2026-05-01.

- [ ] **Step 2: Copy the crate**

Run from the rupu repo root:
```bash
mkdir -p crates/rupu-providers
cp -R /Users/matt/Code/phi-cell/crates/phi-providers/* crates/rupu-providers/
```

- [ ] **Step 3: Adapt the crate `Cargo.toml`**

Edit `crates/rupu-providers/Cargo.toml`:

- Change `name = "phi-providers"` → `name = "rupu-providers"`.
- Confirm all dependencies use `*.workspace = true` (they should already).
- Confirm `[dev-dependencies]` is `tempfile = "3"` (replace with `tempfile.workspace = true`).

The result should look like:

```toml
[package]
name = "rupu-providers"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true

[lints]
workspace = true

[dependencies]
tokio.workspace = true
reqwest.workspace = true
serde.workspace = true
serde_json.workspace = true
tracing.workspace = true
thiserror.workspace = true
async-trait.workspace = true
ed25519-dalek.workspace = true
base64.workspace = true
futures-util.workspace = true
chrono.workspace = true
fs2.workspace = true
toml.workspace = true

[dev-dependencies]
tempfile.workspace = true
```

- [ ] **Step 4: Add to workspace members**

Modify root `Cargo.toml`:

```toml
members = [
    "crates/rupu-transcript",
    "crates/rupu-config",
    "crates/rupu-workspace",
    "crates/rupu-auth",
    "crates/rupu-providers",
]
```

- [ ] **Step 5: Update internal references in lifted code**

The lifted code references `phi-providers` only by its `extern crate` name; renaming the package to `rupu-providers` changes that to `rupu_providers` in any in-tree consumer. Since we have no in-tree consumers in this plan, the only thing to verify is that the lifted code itself doesn't reference `phi_providers` internally.

Run: `grep -rn 'phi_providers\|phi-providers' crates/rupu-providers/`
Expected: no matches (or only in comments/doc-strings, which are fine to leave).

If there are matches in code, replace them: `phi_providers` → `rupu_providers`, `phi-providers` → `rupu-providers` (only in source files, NOT in lock files or generated files).

- [ ] **Step 6: Add origin marker file**

Create `crates/rupu-providers/LIFT_ORIGIN.md`:

```markdown
# Lift origin

This crate was lifted from `Section9Labs/phi-cell` `origin/main` at commit
`3c7394cb1f5a87088954a1ff64fce86303066f55` on 2026-05-01.

## What was changed

- Renamed the package from `phi-providers` to `rupu-providers` in `Cargo.toml`.
- Adapted `[dev-dependencies]` to use the rupu workspace (`tempfile.workspace = true`).
- Replaced any internal references to `phi_providers` with `rupu_providers`.

## Why this is a hard lift, not a fork

We do not plan to re-sync from upstream phi-cell. Once lifted, this crate
evolves independently. If phi-cell's provider stack gets a meaningful
improvement we want to bring back, port it as a deliberate change with its
own commit and PR — not a merge.

## Original module layout (preserved)

- `anthropic.rs` — Anthropic Messages API client
- `openai*.rs` — OpenAI Responses API client (if present)
- `github_copilot.rs` — Copilot client
- `local.rs` — Local model client
- `auth.rs`, `auth/` — provider auth
- `provider.rs`, `provider_id.rs`, `registry.rs` — provider trait + registry
- `model_catalog.rs`, `routing_history.rs`, `sse.rs`, `types.rs` — supporting types
- `broker_client.rs`, `broker_types.rs` — broker integration (may be unused in rupu Slice A)
```

- [ ] **Step 7: Build it**

Run: `cargo build -p rupu-providers`
Expected: PASS. If it doesn't, the most likely causes are:

- A workspace dep version mismatch — fix in root `Cargo.toml`.
- A `phi_providers::` reference left in code — fix with grep+replace from Step 5.
- A test-only dependency missing — add to `[dev-dependencies]` in `crates/rupu-providers/Cargo.toml`.

Resolve each error one at a time, re-running `cargo build -p rupu-providers` between fixes.

- [ ] **Step 8: Run the lifted tests**

Run: `cargo test -p rupu-providers`
Expected: All lifted tests pass (the count depends on what phi-cell has; capture the count in the commit message).

If a test fails because it relies on phi-cell-specific path/env (e.g., test fixture lives in `phi-cell/tests/...`), mark it `#[ignore]` with a comment pointing at this Task and revisit in Plan 2 when we know what's actually needed for the agent loop. Do not delete tests.

- [ ] **Step 9: Commit**

```bash
git add Cargo.toml crates/rupu-providers
git commit -m "feat(providers): lift phi-providers from phi-cell@3c7394cb

Lifted from Section9Labs/phi-cell origin/main commit
3c7394cb1f5a87088954a1ff64fce86303066f55 on 2026-05-01. See
crates/rupu-providers/LIFT_ORIGIN.md for the rename + dependency
adaptations. All upstream tests passing in the new workspace."
```

---

## Phase 2 — Tool harness

### Task 17: `rupu-tools` — crate skeleton + Tool trait

**Files:**
- Create: `crates/rupu-tools/Cargo.toml`
- Create: `crates/rupu-tools/src/lib.rs`
- Create: `crates/rupu-tools/src/tool.rs`
- Modify: `Cargo.toml` (members)

- [ ] **Step 1: Create crate Cargo.toml**

```toml
[package]
name = "rupu-tools"
version.workspace = true
edition.workspace = true
license.workspace = true
repository.workspace = true
rust-version.workspace = true

[lints]
workspace = true

[dependencies]
serde.workspace = true
serde_json.workspace = true
thiserror.workspace = true
tracing.workspace = true
async-trait.workspace = true
tokio = { workspace = true, features = ["process", "io-util", "time", "fs", "macros", "rt-multi-thread"] }

# Tool implementations
which = "6"        # locate ripgrep / rg binary
globwalk = "0.9"   # glob tool

[dev-dependencies]
tempfile.workspace = true
assert_fs.workspace = true
predicates.workspace = true
```

Add `which = "6"` and `globwalk = "0.9"` to root `Cargo.toml` `[workspace.dependencies]` if you prefer; for v0 tool-only crate-local deps are fine.

- [ ] **Step 2: Add to workspace members**

```toml
members = [
    "crates/rupu-transcript",
    "crates/rupu-config",
    "crates/rupu-workspace",
    "crates/rupu-auth",
    "crates/rupu-providers",
    "crates/rupu-tools",
]
```

- [ ] **Step 3: Create the trait**

Create `crates/rupu-tools/src/tool.rs`:

```rust
//! Tool trait. Each tool implements one verb the agent can invoke.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ToolError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("timeout")]
    Timeout,
    #[error("permission denied")]
    PermissionDenied,
    #[error("execution: {0}")]
    Execution(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolContext {
    pub workspace_path: PathBuf,
    pub bash_env_allowlist: Vec<String>,
    pub bash_timeout_secs: u64,
}

impl Default for ToolContext {
    fn default() -> Self {
        Self {
            workspace_path: PathBuf::from("."),
            bash_env_allowlist: Vec::new(),
            bash_timeout_secs: 120,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub stdout: String,
    pub error: Option<String>,
    pub duration_ms: u64,
    /// If the tool corresponds to a derived event (file_edit, command_run),
    /// the runtime emits the derived event in addition to `tool_result`.
    pub derived: Option<DerivedEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum DerivedEvent {
    FileEdit {
        path: String,
        kind: String, // "create" | "modify" | "delete"
        diff: String,
    },
    CommandRun {
        argv: Vec<String>,
        cwd: String,
        exit_code: i32,
        stdout_bytes: u64,
        stderr_bytes: u64,
    },
}

#[async_trait]
pub trait Tool: Send + Sync {
    /// Stable tool name used in agent files and tool calls.
    fn name(&self) -> &'static str;
    /// Invoke the tool with JSON-encoded input.
    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError>;
}
```

Create `crates/rupu-tools/src/lib.rs`:

```rust
//! rupu-tools — six tools (bash, read_file, write_file, edit_file, grep, glob).

pub mod bash;
pub mod edit_file;
pub mod glob;
pub mod grep;
pub mod permission;
pub mod read_file;
pub mod tool;
pub mod write_file;

pub use bash::BashTool;
pub use edit_file::EditFileTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use permission::{PermissionGate, PermissionMode};
pub use read_file::ReadFileTool;
pub use tool::{DerivedEvent, Tool, ToolContext, ToolError, ToolOutput};
pub use write_file::WriteFileTool;
```

Stub the seven tool/permission modules so the crate compiles. Each is a one-line stub like:

```rust
// crates/rupu-tools/src/bash.rs
pub struct BashTool;
```

`crates/rupu-tools/src/permission.rs`:

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    Ask,
    Bypass,
    Readonly,
}

pub struct PermissionGate;
```

For each of `bash.rs`, `read_file.rs`, `write_file.rs`, `edit_file.rs`, `grep.rs`, `glob.rs`, just put a `pub struct XxxTool;` stub.

- [ ] **Step 4: Verify it compiles**

Run: `cargo build -p rupu-tools`
Expected: PASS (warnings about unused are OK).

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml crates/rupu-tools
git commit -m "feat(tools): crate skeleton + Tool trait"
```

---

### Task 18: `rupu-tools` — PermissionGate (TDD, non-interactive paths only)

**Files:**
- Modify: `crates/rupu-tools/src/permission.rs`
- Test: `crates/rupu-tools/tests/permission.rs`

**Note on TTY:** The interactive `ask`-mode prompt UX is tested in Plan 2 with a pty harness. v0 of `PermissionGate` exposes a non-interactive decision API (`decide_for_mode`) that the runtime drives — keeping unit-testable logic separate from terminal I/O.

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-tools/tests/permission.rs`:

```rust
use rupu_tools::{PermissionGate, PermissionMode};

#[test]
fn readonly_denies_writers() {
    let gate = PermissionGate::for_mode(PermissionMode::Readonly);
    assert!(!gate.allow_unconditionally("bash"));
    assert!(!gate.allow_unconditionally("write_file"));
    assert!(!gate.allow_unconditionally("edit_file"));
    assert!(gate.allow_unconditionally("read_file"));
    assert!(gate.allow_unconditionally("grep"));
    assert!(gate.allow_unconditionally("glob"));
}

#[test]
fn bypass_allows_everything() {
    let gate = PermissionGate::for_mode(PermissionMode::Bypass);
    for tool in ["bash", "write_file", "edit_file", "read_file", "grep", "glob"] {
        assert!(gate.allow_unconditionally(tool), "{tool} denied under bypass");
    }
}

#[test]
fn ask_allows_readers_unconditionally() {
    let gate = PermissionGate::for_mode(PermissionMode::Ask);
    assert!(gate.allow_unconditionally("read_file"));
    assert!(gate.allow_unconditionally("grep"));
    assert!(gate.allow_unconditionally("glob"));
}

#[test]
fn ask_requires_decision_for_writers() {
    let gate = PermissionGate::for_mode(PermissionMode::Ask);
    assert!(!gate.allow_unconditionally("bash"));
    assert!(gate.requires_decision("bash"));
    assert!(gate.requires_decision("write_file"));
    assert!(gate.requires_decision("edit_file"));
    assert!(!gate.requires_decision("read_file"));
}

#[test]
fn unknown_tool_is_denied() {
    let gate = PermissionGate::for_mode(PermissionMode::Bypass);
    // Even bypass shouldn't whitelist a tool we don't know about.
    // The runtime would refuse to dispatch it, but the gate also says no.
    assert!(!gate.allow_unconditionally("unknown_tool"));
}
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p rupu-tools --test permission`
Expected: FAIL.

- [ ] **Step 3: Implement**

Replace `crates/rupu-tools/src/permission.rs` with:

```rust
//! Permission gating for tool calls. Three modes; all logic here is
//! synchronous and pure. Interactive prompt UX (for `ask` mode) lives
//! in `rupu-cli` (Plan 2) and consumes this gate.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    Ask,
    Bypass,
    Readonly,
}

#[derive(Debug, Clone, Copy)]
pub struct PermissionGate {
    mode: PermissionMode,
}

const KNOWN_READ_TOOLS: &[&str] = &["read_file", "grep", "glob"];
const KNOWN_WRITE_TOOLS: &[&str] = &["bash", "write_file", "edit_file"];

impl PermissionGate {
    pub fn for_mode(mode: PermissionMode) -> Self {
        Self { mode }
    }

    pub fn mode(&self) -> PermissionMode {
        self.mode
    }

    /// True if the tool can run with no operator decision.
    pub fn allow_unconditionally(&self, tool: &str) -> bool {
        let is_read = KNOWN_READ_TOOLS.contains(&tool);
        let is_write = KNOWN_WRITE_TOOLS.contains(&tool);
        if !is_read && !is_write {
            return false; // unknown tool: never allow without explicit thought
        }
        match self.mode {
            PermissionMode::Bypass => true,
            PermissionMode::Readonly => is_read,
            PermissionMode::Ask => is_read,
        }
    }

    /// True if the tool can run only after the operator decides.
    pub fn requires_decision(&self, tool: &str) -> bool {
        let is_write = KNOWN_WRITE_TOOLS.contains(&tool);
        matches!(self.mode, PermissionMode::Ask) && is_write
    }

    /// True if the tool is denied outright (no decision possible).
    pub fn denied_outright(&self, tool: &str) -> bool {
        let is_write = KNOWN_WRITE_TOOLS.contains(&tool);
        let is_read = KNOWN_READ_TOOLS.contains(&tool);
        if !is_read && !is_write {
            return true;
        }
        matches!(self.mode, PermissionMode::Readonly) && is_write
    }
}
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rupu-tools --test permission`
Expected: 5 passing tests.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-tools
git commit -m "feat(tools): PermissionGate with non-interactive decision API"
```

---

### Task 19: `rupu-tools` — read_file (TDD)

**Files:**
- Modify: `crates/rupu-tools/src/read_file.rs`
- Test: `crates/rupu-tools/tests/read_file.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-tools/tests/read_file.rs`:

```rust
use assert_fs::prelude::*;
use rupu_tools::{ReadFileTool, Tool, ToolContext};
use serde_json::json;

fn ctx(workspace: &std::path::Path) -> ToolContext {
    ToolContext { workspace_path: workspace.to_path_buf(), ..Default::default() }
}

#[tokio::test]
async fn reads_file_with_line_numbers() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let f = tmp.child("hello.txt");
    f.write_str("first\nsecond\nthird\n").unwrap();

    let tool = ReadFileTool;
    let out = tool
        .invoke(json!({ "path": "hello.txt" }), &ctx(tmp.path()))
        .await
        .unwrap();
    assert!(out.stdout.contains("1\tfirst"));
    assert!(out.stdout.contains("2\tsecond"));
    assert!(out.stdout.contains("3\tthird"));
    assert!(out.error.is_none());
}

#[tokio::test]
async fn missing_file_returns_error() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let tool = ReadFileTool;
    let out = tool
        .invoke(json!({ "path": "nope.txt" }), &ctx(tmp.path()))
        .await
        .unwrap();
    assert!(out.error.is_some(), "expected error for missing file");
}

#[tokio::test]
async fn rejects_path_outside_workspace() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let tool = ReadFileTool;
    let out = tool
        .invoke(json!({ "path": "../etc/passwd" }), &ctx(tmp.path()))
        .await;
    // Either err or output-with-error is acceptable; not allowed to read.
    let invalid = match out {
        Err(_) => true,
        Ok(o) => o.error.is_some(),
    };
    assert!(invalid, "must refuse paths escaping workspace");
}

#[tokio::test]
async fn missing_path_input_is_invalid() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let tool = ReadFileTool;
    let res = tool.invoke(json!({}), &ctx(tmp.path())).await;
    assert!(res.is_err());
}
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p rupu-tools --test read_file`
Expected: FAIL — `ReadFileTool` is just a stub struct.

- [ ] **Step 3: Implement**

Replace `crates/rupu-tools/src/read_file.rs` with:

```rust
//! `read_file` tool — full-file read with line-numbered output.

use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;
use std::time::Instant;

#[derive(Deserialize)]
struct Input {
    path: String,
}

#[derive(Debug, Default, Clone)]
pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &'static str { "read_file" }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let i: Input = serde_json::from_value(input)
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let abs = ctx.workspace_path.join(&i.path);
        if !is_inside(&ctx.workspace_path, &abs) {
            return Ok(ToolOutput {
                stdout: String::new(),
                error: Some(format!("path {} escapes workspace", i.path)),
                duration_ms: started.elapsed().as_millis() as u64,
                derived: None,
            });
        }
        match std::fs::read_to_string(&abs) {
            Ok(text) => {
                let mut out = String::with_capacity(text.len() + 64);
                for (idx, line) in text.lines().enumerate() {
                    use std::fmt::Write;
                    writeln!(out, "{}\t{}", idx + 1, line).unwrap();
                }
                Ok(ToolOutput {
                    stdout: out,
                    error: None,
                    duration_ms: started.elapsed().as_millis() as u64,
                    derived: None,
                })
            }
            Err(e) => Ok(ToolOutput {
                stdout: String::new(),
                error: Some(format!("read {}: {e}", i.path)),
                duration_ms: started.elapsed().as_millis() as u64,
                derived: None,
            }),
        }
    }
}

fn is_inside(root: &Path, candidate: &Path) -> bool {
    let Ok(root) = root.canonicalize() else { return false };
    // We canonicalize root only; for missing files we walk components.
    let mut cur = candidate.to_path_buf();
    // Pop one trailing non-existent segment if needed
    if !cur.exists() {
        if let Some(parent) = cur.parent() {
            cur = parent.to_path_buf();
        }
    }
    let Ok(cur) = cur.canonicalize() else { return false };
    cur.starts_with(&root)
}
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rupu-tools --test read_file`
Expected: 4 passing tests.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-tools
git commit -m "feat(tools): read_file with line-numbered output and workspace-scope check"
```

---

### Task 20: `rupu-tools` — write_file (TDD)

**Files:**
- Modify: `crates/rupu-tools/src/write_file.rs`
- Test: `crates/rupu-tools/tests/write_file.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-tools/tests/write_file.rs`:

```rust
use assert_fs::prelude::*;
use rupu_tools::{Tool, ToolContext, WriteFileTool};
use rupu_tools::DerivedEvent;
use serde_json::json;

fn ctx(workspace: &std::path::Path) -> ToolContext {
    ToolContext { workspace_path: workspace.to_path_buf(), ..Default::default() }
}

#[tokio::test]
async fn creates_new_file_and_emits_create_derived() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let out = WriteFileTool
        .invoke(json!({ "path": "new.txt", "content": "hello\n" }), &ctx(tmp.path()))
        .await
        .unwrap();
    assert!(out.error.is_none());
    tmp.child("new.txt").assert("hello\n");
    let derived = out.derived.unwrap();
    let DerivedEvent::FileEdit { kind, .. } = derived else { panic!("expected FileEdit derived"); };
    assert_eq!(kind, "create");
}

#[tokio::test]
async fn overwrites_existing_file_and_emits_modify_derived() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("x.txt").write_str("old\n").unwrap();
    let out = WriteFileTool
        .invoke(json!({ "path": "x.txt", "content": "new\n" }), &ctx(tmp.path()))
        .await
        .unwrap();
    tmp.child("x.txt").assert("new\n");
    let DerivedEvent::FileEdit { kind, .. } = out.derived.unwrap() else { panic!() };
    assert_eq!(kind, "modify");
}

#[tokio::test]
async fn refuses_path_outside_workspace() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let out = WriteFileTool
        .invoke(json!({ "path": "../escape.txt", "content": "x" }), &ctx(tmp.path()))
        .await
        .unwrap();
    assert!(out.error.is_some());
}

#[tokio::test]
async fn creates_intermediate_directories() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let out = WriteFileTool
        .invoke(json!({ "path": "a/b/c.txt", "content": "x" }), &ctx(tmp.path()))
        .await
        .unwrap();
    assert!(out.error.is_none());
    tmp.child("a/b/c.txt").assert("x");
}
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p rupu-tools --test write_file`
Expected: FAIL.

- [ ] **Step 3: Implement**

Replace `crates/rupu-tools/src/write_file.rs` with:

```rust
//! `write_file` tool — create or overwrite a file. Emits a `FileEdit`
//! derived event so the transcript indexes file changes without parsing
//! tool inputs.

use crate::tool::{DerivedEvent, Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;
use std::time::Instant;

#[derive(Deserialize)]
struct Input {
    path: String,
    content: String,
}

#[derive(Debug, Default, Clone)]
pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &'static str { "write_file" }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let i: Input = serde_json::from_value(input)
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let abs = ctx.workspace_path.join(&i.path);
        if !is_inside(&ctx.workspace_path, &abs) {
            return Ok(ToolOutput {
                stdout: String::new(),
                error: Some(format!("path {} escapes workspace", i.path)),
                duration_ms: started.elapsed().as_millis() as u64,
                derived: None,
            });
        }
        let kind = if abs.exists() { "modify" } else { "create" };
        if let Some(parent) = abs.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return Ok(ToolOutput {
                    stdout: String::new(),
                    error: Some(format!("mkdir {}: {e}", parent.display())),
                    duration_ms: started.elapsed().as_millis() as u64,
                    derived: None,
                });
            }
        }
        if let Err(e) = std::fs::write(&abs, &i.content) {
            return Ok(ToolOutput {
                stdout: String::new(),
                error: Some(format!("write {}: {e}", i.path)),
                duration_ms: started.elapsed().as_millis() as u64,
                derived: None,
            });
        }
        Ok(ToolOutput {
            stdout: format!("wrote {} bytes to {}", i.content.len(), i.path),
            error: None,
            duration_ms: started.elapsed().as_millis() as u64,
            derived: Some(DerivedEvent::FileEdit {
                path: i.path,
                kind: kind.to_string(),
                diff: String::new(), // full-content writes; runtime can compute a diff if needed
            }),
        })
    }
}

fn is_inside(root: &Path, candidate: &Path) -> bool {
    let Ok(root) = root.canonicalize() else { return false };
    let mut cur = candidate.to_path_buf();
    if !cur.exists() {
        if let Some(parent) = cur.parent() {
            cur = parent.to_path_buf();
        }
    }
    let Ok(cur) = cur.canonicalize() else { return false };
    cur.starts_with(&root)
}
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rupu-tools --test write_file`
Expected: 4 passing tests.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-tools
git commit -m "feat(tools): write_file with FileEdit derived event"
```

---

### Task 21: `rupu-tools` — edit_file (exact-match replacement) (TDD)

**Files:**
- Modify: `crates/rupu-tools/src/edit_file.rs`
- Test: `crates/rupu-tools/tests/edit_file.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-tools/tests/edit_file.rs`:

```rust
use assert_fs::prelude::*;
use rupu_tools::{DerivedEvent, EditFileTool, Tool, ToolContext};
use serde_json::json;

fn ctx(workspace: &std::path::Path) -> ToolContext {
    ToolContext { workspace_path: workspace.to_path_buf(), ..Default::default() }
}

#[tokio::test]
async fn replaces_exact_match() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("src.txt").write_str("foo\nbar\nbaz\n").unwrap();
    let out = EditFileTool
        .invoke(
            json!({ "path": "src.txt", "old_string": "bar\n", "new_string": "BAR\n" }),
            &ctx(tmp.path()),
        )
        .await
        .unwrap();
    assert!(out.error.is_none(), "edit failed: {:?}", out.error);
    tmp.child("src.txt").assert("foo\nBAR\nbaz\n");
    let DerivedEvent::FileEdit { kind, .. } = out.derived.unwrap() else { panic!() };
    assert_eq!(kind, "modify");
}

#[tokio::test]
async fn fails_when_old_string_not_found() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("src.txt").write_str("foo\n").unwrap();
    let out = EditFileTool
        .invoke(
            json!({ "path": "src.txt", "old_string": "missing", "new_string": "x" }),
            &ctx(tmp.path()),
        )
        .await
        .unwrap();
    assert!(out.error.is_some());
    tmp.child("src.txt").assert("foo\n"); // unchanged
}

#[tokio::test]
async fn fails_when_old_string_is_ambiguous() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("src.txt").write_str("dup\ndup\n").unwrap();
    let out = EditFileTool
        .invoke(
            json!({ "path": "src.txt", "old_string": "dup", "new_string": "x" }),
            &ctx(tmp.path()),
        )
        .await
        .unwrap();
    assert!(out.error.is_some(), "ambiguous match must error");
}

#[tokio::test]
async fn refuses_path_outside_workspace() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let out = EditFileTool
        .invoke(
            json!({ "path": "../etc/x", "old_string": "a", "new_string": "b" }),
            &ctx(tmp.path()),
        )
        .await
        .unwrap();
    assert!(out.error.is_some());
}
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p rupu-tools --test edit_file`
Expected: FAIL.

- [ ] **Step 3: Implement**

Replace `crates/rupu-tools/src/edit_file.rs` with:

```rust
//! `edit_file` tool — exact-match string replacement. Ambiguous matches
//! (more than one occurrence) are an error; callers should pass enough
//! surrounding context to make `old_string` unique.

use crate::tool::{DerivedEvent, Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;
use std::time::Instant;

#[derive(Deserialize)]
struct Input {
    path: String,
    old_string: String,
    new_string: String,
}

#[derive(Debug, Default, Clone)]
pub struct EditFileTool;

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &'static str { "edit_file" }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let i: Input = serde_json::from_value(input)
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let abs = ctx.workspace_path.join(&i.path);
        if !is_inside(&ctx.workspace_path, &abs) {
            return Ok(err_output(started, format!("path {} escapes workspace", i.path)));
        }
        let text = match std::fs::read_to_string(&abs) {
            Ok(t) => t,
            Err(e) => return Ok(err_output(started, format!("read {}: {e}", i.path))),
        };
        let count = text.matches(&i.old_string).count();
        if count == 0 {
            return Ok(err_output(started, format!("old_string not found in {}", i.path)));
        }
        if count > 1 {
            return Ok(err_output(
                started,
                format!("old_string matches {count} places in {}; provide more context", i.path),
            ));
        }
        let new_text = text.replacen(&i.old_string, &i.new_string, 1);
        if let Err(e) = std::fs::write(&abs, &new_text) {
            return Ok(err_output(started, format!("write {}: {e}", i.path)));
        }
        Ok(ToolOutput {
            stdout: format!("edited {}", i.path),
            error: None,
            duration_ms: started.elapsed().as_millis() as u64,
            derived: Some(DerivedEvent::FileEdit {
                path: i.path,
                kind: "modify".into(),
                diff: simple_diff(&i.old_string, &i.new_string),
            }),
        })
    }
}

fn err_output(started: Instant, msg: String) -> ToolOutput {
    ToolOutput {
        stdout: String::new(),
        error: Some(msg),
        duration_ms: started.elapsed().as_millis() as u64,
        derived: None,
    }
}

fn simple_diff(old: &str, new: &str) -> String {
    let mut s = String::new();
    for line in old.lines() {
        s.push_str("- ");
        s.push_str(line);
        s.push('\n');
    }
    for line in new.lines() {
        s.push_str("+ ");
        s.push_str(line);
        s.push('\n');
    }
    s
}

fn is_inside(root: &Path, candidate: &Path) -> bool {
    let Ok(root) = root.canonicalize() else { return false };
    let mut cur = candidate.to_path_buf();
    if !cur.exists() {
        if let Some(parent) = cur.parent() {
            cur = parent.to_path_buf();
        }
    }
    let Ok(cur) = cur.canonicalize() else { return false };
    cur.starts_with(&root)
}
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rupu-tools --test edit_file`
Expected: 4 passing tests.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-tools
git commit -m "feat(tools): edit_file with exact-match + ambiguity error"
```

---

### Task 22: `rupu-tools` — grep (TDD; ripgrep delegate)

**Files:**
- Modify: `crates/rupu-tools/src/grep.rs`
- Test: `crates/rupu-tools/tests/grep.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-tools/tests/grep.rs`:

```rust
use assert_fs::prelude::*;
use rupu_tools::{GrepTool, Tool, ToolContext};
use serde_json::json;

fn ctx(workspace: &std::path::Path) -> ToolContext {
    ToolContext { workspace_path: workspace.to_path_buf(), ..Default::default() }
}

fn skip_if_no_rg() -> bool {
    which::which("rg").is_err()
}

#[tokio::test]
async fn finds_matches_across_files() {
    if skip_if_no_rg() {
        eprintln!("skipping: ripgrep not installed");
        return;
    }
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("a.txt").write_str("foo bar\n").unwrap();
    tmp.child("b.txt").write_str("baz qux\n").unwrap();
    let out = GrepTool
        .invoke(json!({ "pattern": "bar" }), &ctx(tmp.path()))
        .await
        .unwrap();
    assert!(out.error.is_none());
    assert!(out.stdout.contains("a.txt"));
    assert!(out.stdout.contains("foo bar"));
    assert!(!out.stdout.contains("b.txt"));
}

#[tokio::test]
async fn no_matches_returns_empty_stdout() {
    if skip_if_no_rg() { return; }
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("a.txt").write_str("foo\n").unwrap();
    let out = GrepTool
        .invoke(json!({ "pattern": "xyz" }), &ctx(tmp.path()))
        .await
        .unwrap();
    assert!(out.stdout.is_empty());
    assert!(out.error.is_none());
}

#[tokio::test]
async fn invalid_input_errors() {
    if skip_if_no_rg() { return; }
    let tmp = assert_fs::TempDir::new().unwrap();
    let res = GrepTool.invoke(json!({}), &ctx(tmp.path())).await;
    assert!(res.is_err());
}
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p rupu-tools --test grep`
Expected: FAIL.

- [ ] **Step 3: Implement**

Replace `crates/rupu-tools/src/grep.rs` with:

```rust
//! `grep` tool — delegates to the `rg` binary if available, falling back
//! to a clear error otherwise. Why ripgrep: gitignore-aware, fast, and
//! every developer's machine already has it. Reimplementing in v0
//! would be over-engineered for the surface area we need.

use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::process::Stdio;
use std::time::Instant;
use tokio::process::Command;

#[derive(Deserialize)]
struct Input {
    pattern: String,
    #[serde(default)]
    path: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &'static str { "grep" }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let i: Input = serde_json::from_value(input)
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;
        let rg = which::which("rg")
            .map_err(|_| ToolError::Execution("`rg` (ripgrep) not found in PATH".into()))?;

        let search_path = i
            .path
            .as_deref()
            .map(|p| ctx.workspace_path.join(p))
            .unwrap_or_else(|| ctx.workspace_path.clone());

        let out = Command::new(rg)
            .arg("--with-filename")
            .arg("--line-number")
            .arg("--no-heading")
            .arg(&i.pattern)
            .arg(&search_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();

        // ripgrep exit code: 0 = matches, 1 = no matches (not an error), 2 = error
        let error = match out.status.code() {
            Some(0) | Some(1) => None,
            _ => Some(if stderr.is_empty() { "rg failed".into() } else { stderr }),
        };

        Ok(ToolOutput {
            stdout,
            error,
            duration_ms: started.elapsed().as_millis() as u64,
            derived: None,
        })
    }
}
```

- [ ] **Step 4: Run — expect PASS (or skipped if no rg)**

Run: `cargo test -p rupu-tools --test grep`
Expected: 3 passing tests if `rg` is on PATH; otherwise 3 tests print "skipping" and pass trivially.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-tools
git commit -m "feat(tools): grep delegates to ripgrep"
```

---

### Task 23: `rupu-tools` — glob (TDD)

**Files:**
- Modify: `crates/rupu-tools/src/glob.rs`
- Test: `crates/rupu-tools/tests/glob.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-tools/tests/glob.rs`:

```rust
use assert_fs::prelude::*;
use rupu_tools::{GlobTool, Tool, ToolContext};
use serde_json::json;

fn ctx(workspace: &std::path::Path) -> ToolContext {
    ToolContext { workspace_path: workspace.to_path_buf(), ..Default::default() }
}

#[tokio::test]
async fn matches_files_by_pattern() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("a.rs").write_str("").unwrap();
    tmp.child("b.rs").write_str("").unwrap();
    tmp.child("c.txt").write_str("").unwrap();
    let out = GlobTool
        .invoke(json!({ "pattern": "*.rs" }), &ctx(tmp.path()))
        .await
        .unwrap();
    assert!(out.error.is_none());
    assert!(out.stdout.contains("a.rs"));
    assert!(out.stdout.contains("b.rs"));
    assert!(!out.stdout.contains("c.txt"));
}

#[tokio::test]
async fn matches_recursively_with_double_star() {
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("src/lib.rs").write_str("").unwrap();
    tmp.child("src/mod/x.rs").write_str("").unwrap();
    let out = GlobTool
        .invoke(json!({ "pattern": "**/*.rs" }), &ctx(tmp.path()))
        .await
        .unwrap();
    assert!(out.stdout.contains("src/lib.rs"));
    assert!(out.stdout.contains("src/mod/x.rs"));
}

#[tokio::test]
async fn no_matches_returns_empty() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let out = GlobTool
        .invoke(json!({ "pattern": "*.zzz" }), &ctx(tmp.path()))
        .await
        .unwrap();
    assert!(out.stdout.is_empty());
    assert!(out.error.is_none());
}
```

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p rupu-tools --test glob`
Expected: FAIL.

- [ ] **Step 3: Implement**

Replace `crates/rupu-tools/src/glob.rs` with:

```rust
//! `glob` tool — recursive pattern matching via `globwalk`.

use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::time::Instant;

#[derive(Deserialize)]
struct Input {
    pattern: String,
}

#[derive(Debug, Default, Clone)]
pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &'static str { "glob" }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let i: Input = serde_json::from_value(input)
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;

        let walker = globwalk::GlobWalkerBuilder::from_patterns(&ctx.workspace_path, &[&i.pattern])
            .max_depth(64)
            .follow_links(false)
            .build()
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        let mut matches = vec![];
        for entry in walker.flatten() {
            if entry.file_type().is_file() {
                let rel = entry
                    .path()
                    .strip_prefix(&ctx.workspace_path)
                    .unwrap_or(entry.path());
                matches.push(rel.display().to_string());
            }
        }
        matches.sort();

        Ok(ToolOutput {
            stdout: matches.join("\n"),
            error: None,
            duration_ms: started.elapsed().as_millis() as u64,
            derived: None,
        })
    }
}
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rupu-tools --test glob`
Expected: 3 passing tests.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-tools
git commit -m "feat(tools): glob with globwalk recursive matching"
```

---

### Task 24: `rupu-tools` — bash (TDD; timeout, signal handling, env control)

**Files:**
- Modify: `crates/rupu-tools/src/bash.rs`
- Test: `crates/rupu-tools/tests/bash.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/rupu-tools/tests/bash.rs`:

```rust
use rupu_tools::{BashTool, DerivedEvent, Tool, ToolContext};
use serde_json::json;
use std::time::Duration;

fn ctx_with_timeout(secs: u64) -> ToolContext {
    let pwd = std::env::current_dir().unwrap();
    ToolContext {
        workspace_path: pwd,
        bash_env_allowlist: vec![],
        bash_timeout_secs: secs,
    }
}

#[tokio::test]
async fn captures_stdout_and_exit_code() {
    let out = BashTool
        .invoke(json!({ "command": "echo hello" }), &ctx_with_timeout(10))
        .await
        .unwrap();
    assert!(out.stdout.contains("hello"));
    let DerivedEvent::CommandRun { exit_code, .. } = out.derived.unwrap() else { panic!() };
    assert_eq!(exit_code, 0);
}

#[tokio::test]
async fn nonzero_exit_is_not_a_tool_error() {
    let out = BashTool
        .invoke(json!({ "command": "exit 7" }), &ctx_with_timeout(10))
        .await
        .unwrap();
    // The tool itself succeeded; the agent sees the exit code and decides.
    assert!(out.error.is_none());
    let DerivedEvent::CommandRun { exit_code, .. } = out.derived.unwrap() else { panic!() };
    assert_eq!(exit_code, 7);
}

#[tokio::test]
async fn timeout_kills_runaway_process() {
    let started = std::time::Instant::now();
    let out = BashTool
        .invoke(json!({ "command": "sleep 60" }), &ctx_with_timeout(2))
        .await
        .unwrap();
    let elapsed = started.elapsed();
    assert!(elapsed < Duration::from_secs(10), "should have killed at ~2s, took {elapsed:?}");
    assert!(out.error.as_deref().unwrap_or("").contains("timeout"));
}

#[tokio::test]
async fn cwd_is_workspace_path() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let mut ctx = ctx_with_timeout(10);
    ctx.workspace_path = tmp.path().to_path_buf();
    let out = BashTool.invoke(json!({ "command": "pwd" }), &ctx).await.unwrap();
    let canonical = tmp.path().canonicalize().unwrap().display().to_string();
    assert!(
        out.stdout.contains(&canonical),
        "expected cwd to contain {canonical}, got: {}",
        out.stdout
    );
}

#[tokio::test]
async fn env_allowlist_filters_inherited_env() {
    let mut ctx = ctx_with_timeout(10);
    ctx.bash_env_allowlist = vec!["RUPU_TEST_VAR".into()];
    std::env::set_var("RUPU_TEST_VAR", "hello-rupu");
    std::env::set_var("RUPU_DENIED_VAR", "should-not-leak");

    let out = BashTool
        .invoke(json!({ "command": "echo $RUPU_TEST_VAR-$RUPU_DENIED_VAR" }), &ctx)
        .await
        .unwrap();
    // Allowed var leaks in; denied var is empty
    assert!(out.stdout.contains("hello-rupu-"), "got: {}", out.stdout);
    assert!(!out.stdout.contains("should-not-leak"));
}
```

Add `assert_fs.workspace = true` to `[dev-dependencies]` of `rupu-tools` if not already there.

- [ ] **Step 2: Run — expect FAIL**

Run: `cargo test -p rupu-tools --test bash`
Expected: FAIL.

- [ ] **Step 3: Implement**

Replace `crates/rupu-tools/src/bash.rs` with:

```rust
//! `bash` tool — execute a command in the workspace cwd with a controlled
//! environment. Timeout sends SIGTERM then SIGKILL.

use crate::tool::{DerivedEvent, Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::Command;
use tokio::time::timeout;

#[derive(Deserialize)]
struct Input {
    command: String,
}

const ALWAYS_ALLOWED_ENV: &[&str] = &["PATH", "HOME", "USER", "TERM", "LANG"];

#[derive(Debug, Default, Clone)]
pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &'static str { "bash" }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let i: Input = serde_json::from_value(input)
            .map_err(|e| ToolError::InvalidInput(e.to_string()))?;

        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(&i.command);
        cmd.current_dir(&ctx.workspace_path);
        cmd.env_clear();
        for key in ALWAYS_ALLOWED_ENV.iter().chain(ctx.bash_env_allowlist.iter().map(|s| s.as_str())) {
            if let Ok(val) = std::env::var(key) {
                cmd.env(key, val);
            }
        }
        cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
        cmd.kill_on_drop(true);

        let child = cmd.spawn().map_err(|e| ToolError::Execution(e.to_string()))?;
        let timeout_dur = Duration::from_secs(ctx.bash_timeout_secs);

        match timeout(timeout_dur, child.wait_with_output()).await {
            Ok(Ok(out)) => {
                let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
                let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
                let exit_code = out.status.code().unwrap_or(-1);
                let combined = if stderr.is_empty() {
                    stdout.clone()
                } else if stdout.is_empty() {
                    stderr.clone()
                } else {
                    format!("{stdout}\n[stderr]\n{stderr}")
                };
                Ok(ToolOutput {
                    stdout: combined,
                    error: None,
                    duration_ms: started.elapsed().as_millis() as u64,
                    derived: Some(DerivedEvent::CommandRun {
                        argv: vec!["/bin/sh".into(), "-c".into(), i.command],
                        cwd: ctx.workspace_path.display().to_string(),
                        exit_code,
                        stdout_bytes: out.stdout.len() as u64,
                        stderr_bytes: out.stderr.len() as u64,
                    }),
                })
            }
            Ok(Err(e)) => Ok(ToolOutput {
                stdout: String::new(),
                error: Some(format!("wait: {e}")),
                duration_ms: started.elapsed().as_millis() as u64,
                derived: None,
            }),
            Err(_elapsed) => {
                // Timeout. The kill_on_drop above will SIGKILL when the child handle
                // is dropped at the end of this scope.
                Ok(ToolOutput {
                    stdout: String::new(),
                    error: Some(format!("timeout after {}s", ctx.bash_timeout_secs)),
                    duration_ms: started.elapsed().as_millis() as u64,
                    derived: None,
                })
            }
        }
    }
}
```

- [ ] **Step 4: Run — expect PASS**

Run: `cargo test -p rupu-tools --test bash`
Expected: 5 passing tests.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-tools
git commit -m "feat(tools): bash with timeout, env allowlist, CommandRun derived event"
```

---

## Phase 3 — Final verification

### Task 25: Workspace-wide test + clippy + fmt

- [ ] **Step 1: Workspace test**

Run: `cargo test --workspace`
Expected: ALL passing. Document the count in the commit message of the next task.

- [ ] **Step 2: Workspace clippy**

Run: `cargo clippy --workspace --all-targets -- -D warnings`
Expected: zero warnings. If clippy flags style issues, fix them inline rather than allowing — keep `#![deny(clippy::all)]` honest.

- [ ] **Step 3: Workspace fmt**

Run: `cargo fmt --all -- --check`
Expected: zero diff. If anything needs formatting, run `cargo fmt --all` and re-check.

- [ ] **Step 4: Release build smoke**

Run: `cargo build --release --workspace`
Expected: PASS.

- [ ] **Step 5: Tag the foundation milestone**

```bash
git tag -a v0.0.1-foundation -m "Plan 1 complete: foundation libraries for Slice A"
```

(No push; the tag is local until Plan 2 lands.)

---

## What's *not* in this plan (deferred to Plan 2)

These are stated explicitly so an engineer working through this plan doesn't try to land them prematurely:

- **`rupu-agent`** — agent file parsing, agent loop, permission resolver, interactive `ask` prompt UX with TTY detection. (Plan 2 Phase 3.)
- **`rupu-orchestrator`** — workflow YAML parser, linear runner, action protocol validator. (Plan 2 Phase 4.)
- **`rupu-cli`** — clap subcommands, `rupu` binary, `rupu run`/`agent`/`workflow`/`transcript`/`config`/`auth` commands. (Plan 2 Phase 5.)
- **Default agent library** (`fix-bug`, `add-tests`, `review-diff`, `scaffold`, `summarize-diff`) and default workflows. (Plan 2 Phase 6.)
- **Docs:** `README.md`, `docs/spec.md`, `docs/agent-format.md`, `docs/workflow-format.md`, `docs/transcript-schema.md`. (Plan 2 Phase 6.)
- **GitHub Releases workflow** (build matrix → tar.gz → upload). (Plan 2 Phase 7.)
- **Exit-criterion-B smoke** (cargo install on a clean macOS arm64 box, run real agents against a real repo). (Plan 2 Phase 7.)

---

## Self-review notes

This plan was self-reviewed against `docs/superpowers/specs/2026-05-01-rupu-slice-a-design.md` covering:

- **Spec coverage:** every Slice A spec section that maps to foundation libraries has a task. Sections deferred to Plan 2 are listed in "What's not in this plan."
- **Placeholder scan:** no TBDs, no "implement later," no "similar to task N." Every code step has the actual code.
- **Type consistency:** `Event`, `RunStatus`, `Workspace`, `AuthBackend`, `ProviderId`, `ToolContext`, `ToolOutput`, `DerivedEvent`, `PermissionMode`, `PermissionGate` — names used consistently across tasks.
- **TDD discipline:** every TDD task follows write-failing-test → run-fail → implement → run-pass → commit.

Open assumption to verify in Plan 2: the `rupu-providers` lift may require additional adapter work to bridge to rupu's `Tool` and transcript event types. If so, Plan 2 will add an adapter layer in `rupu-agent` rather than retrofitting `rupu-providers`. This keeps the lift clean and the rupu-specific normalization in the layer that owns it.
