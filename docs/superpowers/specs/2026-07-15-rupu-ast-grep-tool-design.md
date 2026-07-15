# rupu `ast_grep` built-in tool — design

**Date:** 2026-07-15
**Status:** approved (brainstorm)
**Scope:** one new read-only, model-facing built-in tool. No CLI subcommand.

## Motivation

rupu's agent runtime can search code only textually today (`grep` shells out to
`rg`). Regex can't reliably match on syntax — "every `impl Tool for _`", "every
`async fn` returning `Result`", "all call sites of `foo` passing a closure". A
structural search tool lets the model query code by its syntax tree instead of
its bytes, so it can understand and navigate a codebase far more precisely.

We expose the capability of [ast-grep](https://github.com/ast-grep/ast-grep):
tree-sitter-backed structural pattern matching with metavariables, across 20+
languages, via a single CLI binary.

## Approach

**Wrap the `ast-grep` binary** — shell out, exactly mirroring the existing `grep`
tool's `rg` wrapper. Chosen over embedding tree-sitter grammar crates in-process
(large dependency surface, reimplements the binary, contradicts the grep→rg
precedent) and over building a persistent on-disk AST index (much larger, stale-
cache invalidation, and it's symbol-navigation — a different feature than
structural grep).

## Tool contract

- **Name:** `ast_grep`
- **Kind:** read-only, model-facing built-in (no `rupu` subcommand).
- **Input schema** (hand-written `serde_json` JSON Schema, per the tool
  convention):
  - `pattern` — string, **required**. Structural pattern in ast-grep syntax.
    Metavariables: `$VAR` matches a single named node; `$$$` matches zero or more
    nodes. Examples: `impl Tool for $T`, `async fn $NAME($$$) -> Result<$$$>`.
  - `lang` — string, **required**. Grammar to parse the pattern and target files
    with (`rust`, `python`, `typescript`, `go`, …). Required: a pattern is
    ambiguous without knowing which grammar parses it.
  - `path` — string, optional. Search scope, resolved relative to
    `ctx.workspace_path`; defaults to the whole workspace. Same semantics and
    scoping as the `grep` tool.
- **Output:** compact, grep-style lines `path:line:col: <matched snippet>`,
  reformatted from ast-grep's `--json` stream so the model sees a stable,
  machine-friendly shape consistent with the `grep` tool's `path:line:match`
  contract. Empty output means no matches. (Decision: reformatted grep-style
  over passing raw JSON through — smaller, consistent, easier for the model.)
- **Coverage:** emits one `FileTouchEvent::Grep` coverage event per matched file
  via `emit(ctx, ...)`, exactly like the `grep` tool.

## Implementation & wiring

- **New file** `crates/rupu-tools/src/ast_grep.rs`:
  - `#[derive(Deserialize)] struct Input { pattern: String, lang: String,
    #[serde(default)] path: Option<String> }`.
  - `AstGrepTool` unit struct implementing `Tool` (`name`/`description`/
    `input_schema`/`invoke`), structured as a carbon copy of `grep.rs`.
  - `invoke()`: deserialize input → `which::which("ast-grep")` → join `path`
    under `ctx.workspace_path` → run
    `ast-grep run --pattern <p> --lang <l> --json=stream <path>` via
    `tokio::process::Command` → parse the JSON stream into `path:line:col: text`
    lines → emit per-file coverage events → return
    `ToolOutput { stdout, error, duration_ms, derived: None }`.
  - Binary name is `ast-grep` only. We do **not** fall back to the `sg` alias —
    it collides with a system tool (`scutil`-managed `sg`) on macOS.
- **Export** `AstGrepTool` from `crates/rupu-tools/src/lib.rs`.
- **Register** one line in `default_tool_registry()`
  (`crates/rupu-agent/src/tool_registry.rs`):
  `r.insert("ast_grep", Arc::new(AstGrepTool));`.
- **Permission:** add `"ast_grep"` to `KNOWN_READ_TOOLS`
  (`crates/rupu-tools/src/permission.rs`). No CLI decider changes — the
  `ReadonlyDecider` / `AskDecider` gate only the three writers and auto-allow
  readers.
- **Description text** shown to the model explains: structural (syntactic)
  search; requires `pattern` + `lang`; the `$VAR` / `$$$` metavariables; output
  is `path:line:col: match`, empty on no match; and gives one worked example.

## Errors & edge cases

- **Binary missing** (`which::which("ast-grep")` fails): return
  `ToolOutput.error` (not `Err` — tool-internal failures are inline per the
  `ToolOutput` contract) with an install hint:
  `ast-grep not found; install with 'brew install ast-grep' or 'cargo install ast-grep'`.
- **Invalid pattern / unknown lang:** ast-grep exits non-zero → surface its
  stderr as `ToolOutput.error`.
- **No matches:** ast-grep exits 0 with empty stdout → return empty output
  (grep-parity: empty, not an error).
- **Non-UTF8 / parse failures on individual files:** ast-grep skips them; we pass
  through whatever it emits.

## Testing

- Unit tests mirror `grep.rs`'s tests: run `AstGrepTool` against a small fixture
  tree, assert on the reformatted output and on emitted coverage events.
- **Guard:** tests skip (early-return, not fail) when `ast-grep` is not on
  `PATH`. It is a new, not-yet-ubiquitous prerequisite; CI must not go red where
  it is absent.
- **Follow-up task (tracked in the plan):** install `ast-grep` in CI so the
  guarded tests actually execute there.

## Out of scope for v1 (YAGNI)

- Rewrite / autofix mode (`--rewrite`) — would make this a write tool and pull in
  permission-decider changes.
- YAML rule files (`ast-grep scan` with a ruleset).
- Any persistent or cached AST index.
- A human-facing `rupu ast-grep` subcommand.

Each of these is an additive, independent follow-up if wanted later.
