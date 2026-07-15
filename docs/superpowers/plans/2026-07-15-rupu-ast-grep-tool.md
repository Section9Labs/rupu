# `ast_grep` Built-in Tool Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a read-only, model-facing built-in tool `ast_grep` that gives the agent structural (syntax-tree) code search by wrapping the `ast-grep` binary.

**Architecture:** Carbon-copy of the existing `grep` tool (`crates/rupu-tools/src/grep.rs`), which shells out to `rg`. The new tool shells out to `ast-grep run --pattern <p> --lang <l> --json=stream <path>`, parses the JSON-Lines output into compact `path:line:col: match` lines, emits per-file coverage events, is classified read-only in the permission gate, and is registered in the default tool registry.

**Tech Stack:** Rust 2021, `tokio` (async subprocess), `async-trait`, `serde_json`, `which`, `chrono`. External runtime prerequisite: the `ast-grep` binary (v0.44+).

## Global Constraints

- Rust 2021; MSRV pinned in `rust-toolchain.toml`. Do NOT run workspace-wide `cargo fmt` — main is fmt-dirty under the pinned toolchain; format only files you create/edit.
- Workspace deps only — versions pinned in root `Cargo.toml`, never in crate `Cargo.toml`. (This plan adds NO new crate dependencies — `which`, `serde_json`, `tokio`, `chrono`, `async-trait` are already `rupu-tools` deps used by `grep.rs`.)
- `#![deny(clippy::all)]` workspace-wide; `unsafe_code` forbidden.
- Errors: `thiserror` in libraries. Tool-internal failures (binary missing, non-zero exit) are returned inline as `ToolOutput.error: Some(..)`, NOT as `Err`. `ToolError` is reserved for dispatch-boundary failures (`InvalidInput` for bad input JSON, `Execution` for spawn failure).
- Binary name is `ast-grep` ONLY — never fall back to the `sg` alias (collides with a macOS system tool).
- ast-grep exit codes (verified v0.44.1): `0` = matches, `1` = no matches (success), `2`+ = error. Treat `0` and `1` as success — identical to ripgrep.
- Spec: `docs/superpowers/specs/2026-07-15-rupu-ast-grep-tool-design.md`.

---

### Task 1: `AstGrepTool` implementation + tests

The core of the feature: the tool struct, its `Tool` impl (subprocess call + JSON-Lines parsing + coverage emission), the module export, and integration tests. Self-contained and independently testable — a reviewer can accept/reject the tool in isolation before it is wired into the registry.

**Files:**
- Create: `crates/rupu-tools/src/ast_grep.rs`
- Modify: `crates/rupu-tools/src/lib.rs` (add `pub mod ast_grep;` and `pub use ast_grep::AstGrepTool;`)
- Test: `crates/rupu-tools/tests/ast_grep.rs`

**Interfaces:**
- Consumes: `crate::tool::{Tool, ToolContext, ToolError, ToolOutput}`; `crate::coverage_emit::{attribution_from, emit}`; `rupu_coverage::FileTouchEvent`.
- Produces: `pub struct AstGrepTool;` implementing `Tool` with `name() == "ast_grep"`. Registered by name `"ast_grep"` in Task 3; classified read-only in Task 2.

- [ ] **Step 1: Write the failing integration test**

Create `crates/rupu-tools/tests/ast_grep.rs`:

```rust
use assert_fs::prelude::*;
use rupu_tools::{AstGrepTool, Tool, ToolContext};
use serde_json::json;

fn ctx(workspace: &std::path::Path) -> ToolContext {
    ToolContext {
        workspace_path: workspace.to_path_buf(),
        ..Default::default()
    }
}

fn skip_if_no_ast_grep() -> bool {
    which::which("ast-grep").is_err()
}

#[tokio::test]
async fn finds_structural_matches() {
    if skip_if_no_ast_grep() {
        eprintln!("skipping: ast-grep not installed");
        return;
    }
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("x.rs")
        .write_str("fn main() {\n    println!(\"hi\");\n}\nfn helper() {}\n")
        .unwrap();
    let out = AstGrepTool
        .invoke(
            json!({ "pattern": "fn $NAME() { $$$ }", "lang": "rust" }),
            &ctx(tmp.path()),
        )
        .await
        .unwrap();
    assert!(out.error.is_none(), "unexpected error: {:?}", out.error);
    // Compact grep-style, workspace-relative path, 1-based line/col.
    assert!(out.stdout.contains("x.rs:1:1:"), "stdout was: {}", out.stdout);
    assert!(out.stdout.contains("x.rs:4:1:"), "stdout was: {}", out.stdout);
    // Absolute paths must be stripped to workspace-relative.
    assert!(!out.stdout.contains(tmp.path().to_str().unwrap()));
}

#[tokio::test]
async fn no_matches_returns_empty_stdout() {
    if skip_if_no_ast_grep() {
        return;
    }
    let tmp = assert_fs::TempDir::new().unwrap();
    tmp.child("x.rs").write_str("fn main() {}\n").unwrap();
    let out = AstGrepTool
        .invoke(
            json!({ "pattern": "struct $X {}", "lang": "rust" }),
            &ctx(tmp.path()),
        )
        .await
        .unwrap();
    assert!(out.stdout.is_empty(), "stdout was: {}", out.stdout);
    assert!(out.error.is_none());
}

#[tokio::test]
async fn missing_pattern_is_invalid_input() {
    if skip_if_no_ast_grep() {
        return;
    }
    let tmp = assert_fs::TempDir::new().unwrap();
    let res = AstGrepTool
        .invoke(json!({ "lang": "rust" }), &ctx(tmp.path()))
        .await;
    assert!(res.is_err());
}

#[tokio::test]
async fn missing_lang_is_invalid_input() {
    if skip_if_no_ast_grep() {
        return;
    }
    let tmp = assert_fs::TempDir::new().unwrap();
    let res = AstGrepTool
        .invoke(json!({ "pattern": "fn $N() { $$$ }" }), &ctx(tmp.path()))
        .await;
    assert!(res.is_err());
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rupu-tools --test ast_grep`
Expected: FAIL to COMPILE — `unresolved import rupu_tools::AstGrepTool` (the type does not exist yet).

- [ ] **Step 3: Write the tool implementation**

Create `crates/rupu-tools/src/ast_grep.rs`:

```rust
//! `ast_grep` tool — structural (syntax-tree) code search. Delegates
//! to the `ast-grep` binary if available, falling back to a clear
//! error otherwise.
//!
//! Why ast-grep: tree-sitter-backed pattern matching across 20+
//! languages via one binary. Reimplementing tree-sitter in-process
//! would be a large dependency surface for a capability the binary
//! already provides — this mirrors the `grep` tool's `rg` wrapper.
//!
//! Binary name is `ast-grep` only. We do NOT fall back to the `sg`
//! alias: it collides with a system tool on macOS.
//!
//! Exit-code semantics (match ripgrep): 0 = matches, 1 = no matches
//! (NOT an error), 2+ = real failure. We treat 0 and 1 as success;
//! anything else surfaces stderr in `error`.

use crate::coverage_emit::{attribution_from, emit};
use crate::tool::{Tool, ToolContext, ToolError, ToolOutput};
use async_trait::async_trait;
use chrono::Utc;
use rupu_coverage::FileTouchEvent;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::process::Stdio;
use std::time::Instant;
use tokio::process::Command;

#[derive(Deserialize)]
struct Input {
    /// Structural pattern in ast-grep syntax. Metavariables: `$VAR`
    /// matches one named node, `$$$` matches zero or more nodes.
    pattern: String,
    /// Grammar to parse the pattern and target files with (e.g. `rust`,
    /// `python`, `typescript`). Required — a pattern is ambiguous
    /// without a grammar.
    lang: String,
    /// Optional sub-path within the workspace; defaults to workspace
    /// root.
    #[serde(default)]
    path: Option<String>,
}

/// Workspace-scoped structural search that delegates to `ast-grep`.
#[derive(Debug, Default, Clone)]
pub struct AstGrepTool;

#[async_trait]
impl Tool for AstGrepTool {
    fn name(&self) -> &'static str {
        "ast_grep"
    }

    fn description(&self) -> &'static str {
        "Search the workspace by code STRUCTURE (syntax tree), not text, using ast-grep. \
Provide a `pattern` in ast-grep syntax and a `lang` (rust, python, typescript, go, …). \
Metavariables: `$VAR` matches one named node, `$$$` matches zero or more nodes. \
Example: pattern `impl $T for $S` with lang `rust` finds trait impls; \
pattern `async fn $NAME($$$) -> Result<$$$>` finds async fns returning Result. \
Output is `path:line:col: match` lines (1-based, workspace-relative). \
Prefer this over `grep` when you want syntactic matches (call sites, impls, \
signatures) instead of regex over raw text. Returns empty stdout (not an error) \
when there are no matches."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Structural pattern in ast-grep syntax. Metavariables: `$VAR` = one node, `$$$` = zero-or-more nodes. Example: `impl $T for $S`."
                },
                "lang": {
                    "type": "string",
                    "description": "Language grammar to parse with, e.g. `rust`, `python`, `typescript`, `go`, `javascript`, `java`, `c`, `cpp`. Required."
                },
                "path": {
                    "type": "string",
                    "description": "Optional sub-path within the workspace to restrict the search. Defaults to the whole workspace."
                }
            },
            "required": ["pattern", "lang"]
        })
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        let started = Instant::now();
        let i: Input =
            serde_json::from_value(input).map_err(|e| ToolError::InvalidInput(e.to_string()))?;

        let ast_grep = match which::which("ast-grep") {
            Ok(p) => p,
            Err(_) => {
                return Ok(ToolOutput {
                    stdout: String::new(),
                    error: Some(
                        "ast-grep not found; install with 'brew install ast-grep' or 'cargo install ast-grep'".into(),
                    ),
                    duration_ms: started.elapsed().as_millis() as u64,
                    derived: None,
                });
            }
        };

        let search_path = i
            .path
            .as_deref()
            .map(|p| ctx.workspace_path.join(p))
            .unwrap_or_else(|| ctx.workspace_path.clone());

        let out = Command::new(ast_grep)
            .arg("run")
            .arg("--pattern")
            .arg(&i.pattern)
            .arg("--lang")
            .arg(&i.lang)
            .arg("--json=stream")
            .arg(&search_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .output()
            .await
            .map_err(|e| ToolError::Execution(e.to_string()))?;

        let raw_stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();

        // ast-grep exit code: 0 = matches, 1 = no matches (success),
        // 2+ = error. Mirror ripgrep handling.
        let error = match out.status.code() {
            Some(0) | Some(1) => None,
            _ => Some(if stderr.is_empty() {
                "ast-grep failed".into()
            } else {
                stderr
            }),
        };

        // On success, parse the JSON-Lines stream into compact
        // `path:line:col: <first line of match>` output and per-file
        // coverage events. `--json=stream` emits one JSON object per
        // match; line/column are 0-based, so we add 1.
        let mut stdout = String::new();
        if error.is_none() {
            let mut by_file: BTreeMap<String, Vec<u32>> = BTreeMap::new();
            for raw_line in raw_stdout.lines() {
                let obj: Value = match serde_json::from_str(raw_line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let raw_path = obj.get("file").and_then(Value::as_str).unwrap_or("");
                if raw_path.is_empty() {
                    continue;
                }
                let start = obj.get("range").and_then(|r| r.get("start"));
                let line0 = start.and_then(|s| s.get("line")).and_then(Value::as_u64);
                let col0 = start.and_then(|s| s.get("column")).and_then(Value::as_u64);
                let (Some(line0), Some(col0)) = (line0, col0) else {
                    continue;
                };
                let line = (line0 as u32) + 1;
                let col = (col0 as u32) + 1;

                // First line of the (possibly multi-line) matched text.
                let snippet = obj
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .lines()
                    .next()
                    .unwrap_or("");

                // Make the path workspace-relative if possible.
                let rel_path = std::path::Path::new(raw_path)
                    .strip_prefix(&ctx.workspace_path)
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|_| raw_path.to_string());

                stdout.push_str(&format!("{rel_path}:{line}:{col}: {snippet}\n"));
                by_file.entry(rel_path).or_default().push(line);
            }

            for (path, matched_lines) in by_file {
                let match_count = matched_lines.len() as u32;
                emit(
                    ctx,
                    FileTouchEvent::Grep {
                        path,
                        pattern: i.pattern.clone(),
                        match_count,
                        matched_lines,
                        tool: "ast_grep".to_string(),
                        attribution: attribution_from(ctx),
                        at: Utc::now(),
                    },
                )
                .await;
            }
        }

        Ok(ToolOutput {
            stdout,
            error,
            duration_ms: started.elapsed().as_millis() as u64,
            derived: None,
        })
    }
}
```

- [ ] **Step 4: Wire the module into the crate**

In `crates/rupu-tools/src/lib.rs`, add the module declaration next to the other tool modules (after the `pub mod grep;` block, around line 29):

```rust
// structural (tree-sitter) search — delegates to the `ast-grep` binary.
pub mod ast_grep;
```

And add the re-export alongside the others (the `pub use` block, keep alphabetical — before `pub use bash::BashTool;` at line 39):

```rust
pub use ast_grep::AstGrepTool;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p rupu-tools --test ast_grep`
Expected: PASS — 4 tests pass (they execute because `ast-grep` is installed on this machine; they would skip-and-pass if it were absent).

- [ ] **Step 6: Lint the new file**

Run: `cargo clippy -p rupu-tools --tests`
Expected: no warnings on `ast_grep.rs` / `tests/ast_grep.rs`.
Then format ONLY the files you touched (never workspace-wide):
`rustfmt crates/rupu-tools/src/ast_grep.rs crates/rupu-tools/tests/ast_grep.rs`

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-tools/src/ast_grep.rs crates/rupu-tools/src/lib.rs crates/rupu-tools/tests/ast_grep.rs
git commit -m "feat(tools): add ast_grep structural-search tool"
```

---

### Task 2: Classify `ast_grep` as a read-only tool

The permission gate hard-codes read/write tool lists; an unlisted tool is denied even under Bypass. Classify `ast_grep` as a reader. Separable reviewer gate: this is the security-relevant classification, distinct from the tool logic (Task 1) and the catalog wiring (Task 3).

**Files:**
- Modify: `crates/rupu-tools/src/permission.rs:32` (`KNOWN_READ_TOOLS`)
- Test: `crates/rupu-tools/tests/permission.rs`

**Interfaces:**
- Consumes: `AstGrepTool`'s name `"ast_grep"` (Task 1).
- Produces: `PermissionGate::allow_unconditionally("ast_grep") == true` under all three modes. No CLI decider change needed — `ReadonlyDecider`/`AskDecider` gate only the three writers and auto-allow everything else.

- [ ] **Step 1: Add the failing test assertions**

In `crates/rupu-tools/tests/permission.rs`, extend the existing `readonly_denies_writers` test with an `ast_grep` reader assertion (add after the `glob` line, currently line 11):

```rust
    assert!(gate.allow_unconditionally("ast_grep"));
```

And add a new focused test at the end of the file:

```rust
#[test]
fn ast_grep_is_a_reader() {
    for mode in [
        PermissionMode::Ask,
        PermissionMode::Bypass,
        PermissionMode::Readonly,
    ] {
        let gate = PermissionGate::for_mode(mode);
        assert!(
            gate.allow_unconditionally("ast_grep"),
            "ast_grep should be allowed under {mode:?}"
        );
        assert!(!gate.requires_decision("ast_grep"));
        assert!(!gate.denied_outright("ast_grep"));
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p rupu-tools --test permission ast_grep_is_a_reader`
Expected: FAIL — `ast_grep should be allowed under Ask` (the tool is unknown, so `allow_unconditionally` returns false).

- [ ] **Step 3: Add `ast_grep` to the read-tool list**

In `crates/rupu-tools/src/permission.rs`, line 32, change:

```rust
const KNOWN_READ_TOOLS: &[&str] = &["read_file", "grep", "glob"];
```

to:

```rust
const KNOWN_READ_TOOLS: &[&str] = &["read_file", "grep", "glob", "ast_grep"];
```

- [ ] **Step 4: Run the permission tests to verify they pass**

Run: `cargo test -p rupu-tools --test permission`
Expected: PASS — all permission tests pass, including `ast_grep_is_a_reader`.

- [ ] **Step 5: Commit**

```bash
git add crates/rupu-tools/src/permission.rs crates/rupu-tools/tests/permission.rs
git commit -m "feat(tools): classify ast_grep as a read-only tool"
```

---

### Task 3: Register `ast_grep` in the default tool registry

Wire the tool into the catalog the agent loop sends to the model, and update the enumeration tests that assert the exact tool set. Separable reviewer gate: this is the last mile that makes the tool reachable by the model.

**Files:**
- Modify: `crates/rupu-agent/src/tool_registry.rs` (import at line 7-10; `default_tool_registry()` at line 75-89)
- Test: `crates/rupu-agent/tests/tool_registry.rs` (fixed lists at ~line 30 and ~line 66)

**Interfaces:**
- Consumes: `rupu_tools::AstGrepTool` (Task 1); classified read-only (Task 2).
- Produces: `default_tool_registry().get("ast_grep").is_some()`; `to_tool_definitions()` now yields 9 tools including `ast_grep`, so the model sees it.

- [ ] **Step 1: Update the failing enumeration tests**

In `crates/rupu-agent/tests/tool_registry.rs`:

(a) In `known_tools_returns_sorted_list`, add `"ast_grep"` to the expected sorted vec — it sorts first alphabetically (before `"bash"`):

```rust
    assert_eq!(
        names,
        vec![
            "ast_grep",
            "bash",
            "dispatch_agent",
            "dispatch_agents_parallel",
            "edit_file",
            "glob",
            "grep",
            "read_file",
            "write_file",
        ]
    );
```

(b) In `to_tool_definitions_returns_all_default_tools`, bump the count and add `"ast_grep"` to the sorted vec:

```rust
    let defs = r.to_tool_definitions();
    assert_eq!(defs.len(), 9);
    let mut names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
    names.sort();
    assert_eq!(
        names,
        vec![
            "ast_grep",
            "bash",
            "dispatch_agent",
            "dispatch_agents_parallel",
            "edit_file",
            "glob",
            "grep",
            "read_file",
            "write_file",
        ]
    );
```

(c) Add a focused test at the end of the file:

```rust
#[test]
fn default_registry_contains_ast_grep() {
    let r = default_tool_registry();
    assert!(r.get("ast_grep").is_some());
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p rupu-agent --test tool_registry`
Expected: FAIL — `default_registry_contains_ast_grep` fails (not registered) and the two enumeration tests fail on length/vec mismatch.

- [ ] **Step 3: Register the tool**

In `crates/rupu-agent/src/tool_registry.rs`:

(a) Add `AstGrepTool` to the import list (line 7-10):

```rust
use rupu_tools::{
    AstGrepTool, BashTool, DispatchAgentTool, DispatchAgentsParallelTool, EditFileTool, GlobTool,
    GrepTool, ReadFileTool, Tool, WriteFileTool,
};
```

(b) In `default_tool_registry()`, add the insert next to `grep` (after line 81):

```rust
    r.insert("ast_grep", Arc::new(AstGrepTool));
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rupu-agent --test tool_registry`
Expected: PASS — all registry tests pass, including `default_registry_contains_ast_grep`.

- [ ] **Step 5: Lint and format touched files**

Run: `cargo clippy -p rupu-agent --tests`
Expected: no warnings.
Then: `rustfmt crates/rupu-agent/src/tool_registry.rs crates/rupu-agent/tests/tool_registry.rs`

- [ ] **Step 6: Full build + test sweep for the two crates**

Run: `cargo test -p rupu-tools -p rupu-agent`
Expected: PASS — no regressions across both crates.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-agent/src/tool_registry.rs crates/rupu-agent/tests/tool_registry.rs
git commit -m "feat(agent): register ast_grep in the default tool registry"
```

---

## Self-Review

**1. Spec coverage:**
- Tool contract (name/input/output) → Task 1 (Steps 1, 3). ✓
- Shells out to `ast-grep run --pattern --lang --json=stream`, workspace-scoped, `which::which("ast-grep")`, no `sg` fallback → Task 1 Step 3. ✓
- Grep-style reformatted output, 1-based line/col, workspace-relative paths, first line of multi-line match → Task 1 Step 3 + assertions in Step 1. ✓
- `FileTouchEvent::Grep` coverage per matched file → Task 1 Step 3. ✓
- Export from `lib.rs`, register in `default_tool_registry()` → Task 1 Step 4, Task 3 Step 3. ✓
- Add to `KNOWN_READ_TOOLS`; no CLI decider change → Task 2. ✓
- Binary-missing → inline `ToolOutput.error` with install hint → Task 1 Step 3. ✓
- Exit-code semantics (0/1 success, 2+ error) → Task 1 Step 3. ✓
- Tests skip when `ast-grep` absent → Task 1 Step 1 (`skip_if_no_ast_grep`). ✓
- Enumeration tests updated → Task 3 Step 1. ✓
- Out-of-scope items (rewrite, YAML rules, index, subcommand) → not implemented, correct. ✓

**2. Placeholder scan:** No TBD/TODO; every code step shows complete code; no "similar to Task N" hand-waves. ✓

**3. Type consistency:** `AstGrepTool` (unit struct) used identically in Task 1 (`.invoke`), Task 2 (name string `"ast_grep"`), Task 3 (`Arc::new(AstGrepTool)`). Tool name string `"ast_grep"` consistent across `name()`, `KNOWN_READ_TOOLS`, registry insert, and all test assertions. `FileTouchEvent::Grep` field names (`path`, `pattern`, `match_count`, `matched_lines`, `tool`, `attribution`, `at`) copied verbatim from `grep.rs`. ✓
