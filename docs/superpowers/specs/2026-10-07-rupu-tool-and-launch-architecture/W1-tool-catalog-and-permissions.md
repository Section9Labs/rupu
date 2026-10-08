# W1: Tool descriptor, catalog and one permission policy

- **Card:** W1 · **Depends on:** nothing · **Blocks:** W2, W3, W4
- **Principles:** P1, P3 · **Decisions:** D3, D4, D5, D6, D7, D11
- **Fixes:** T2, T3, T4, T5, T6, T15 (names), T16, T17, R2

## 1. Goal

Every tool carries one `ToolDescriptor` that states what the tool *is*, including the effect it has on the world. One `PermissionPolicy` decides every call from that effect and the run's mode. This replaces the four classifications and three gates, and closes the readonly/ask holes, before anything else moves.

W1 does **not** move tool bodies between crates. W4 and W5 do that. W1 changes the `Tool` trait so that every implementation, wherever it currently lives, declares a descriptor.

## 2. Today

```mermaid
flowchart LR
  call["tool_use"] --> D{"PermissionDecider<br/>(4 impls)"}
  D -- "name ∈ {bash, write_file, edit_file}?" --> W["treated as write"]
  D -- "anything else" --> R["treated as read<br/>(dispatch_agent, finding.verify,<br/>board.post, run_workflow...)"]
  call --> M{"McpPermission<br/>(MCP tools only)"}
  M -- "ToolKind::Write + Readonly" --> deny
  M -- "Ask" --> allow["allow, never prompts<br/>(ask_cb never set)"]
  G["PermissionGate<br/>KNOWN_READ/WRITE_TOOLS"] -. "tests only" .-> x[" "]
```

- `PermissionDecider` impls: `BypassDecider` (`rupu-agent/src/runner.rs:1302`), `ReadonlyDecider` (`runner.rs:1325`, and a duplicate at `rupu-cli/src/cmd/run.rs:1803` used by sessions), and `AskDecider` (`cmd/run.rs:1887`, which ignores its `_mode` argument).
- "Allow always" (`AllowAlwaysForToolThisRun`) sets `runtime_mode = Bypass` for the whole run (`runner.rs:2950`).
- Sub-agents always get `BypassDecider` (`cmd/dispatch.rs:477`), and their `mode_str` lets the child's `permissionMode` override the parent's (`:422`).
- Mode parsing happens in three places: `permission::parse_mode` (returns `Option`), plus `parse_mode_for_runtime` and `parse_mode_for_event` (`runner.rs:3396/3410`) which both default to Ask. Launch sites also compare `== "readonly"` directly.

## 3. Design

### 3.1 Types (`crates/rupu-tools/src/descriptor.rs`, new)

```rust
/// What a tool does to the world. Exactly one per tool. Permission is a
/// pure function of this and the run's mode (no permission code reads names).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// Observes only: files, ledgers, catalogs, status. (read_file, grep, findings.query)
    Read,
    /// Writes rupu's own run bookkeeping, never the user's workspace or the
    /// outside world. (findings.report, coverage.mark, board.post, msg.send)
    Record,
    /// Mutates the workspace or runs arbitrary code. (write_file, edit_file, bash)
    Write,
    /// Acts on a system outside this machine. (scm.prs.create, issues.comment,
    /// github.workflows_dispatch)
    External,
    /// Starts another run or a model call. The child inherits a permission
    /// ceiling. (dispatch, workflows.generate)
    Spawn,
}

/// A service a tool reads from `ToolServices` (W3 introduces the struct;
/// W1 only declares the enum so descriptors are final now).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Service {
    Launcher, Scm, Findings, Coverage, MessageBus, RunStatus,
    Catalog, WorkflowGenerator, Netflow,
}

pub struct ToolDescriptor {
    pub name: &'static str,            // canonical: "findings.report", or core "bash"
    pub aliases: &'static [&'static str], // legacy names: &["report_finding"]
    pub effect: Effect,
    pub needs: &'static [Service],
    pub description: &'static str,
    pub input_schema: fn() -> serde_json::Value,
}
```

### 3.2 The `Tool` trait

```rust
#[async_trait]
pub trait Tool: Send + Sync {
    fn descriptor(&self) -> &'static ToolDescriptor;
    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError>;

    // Provided, so existing call sites keep compiling during the migration:
    fn name(&self) -> &'static str { self.descriptor().name }
    fn description(&self) -> &'static str { self.descriptor().description }
    fn input_schema(&self) -> Value { (self.descriptor().input_schema)() }
}
```

`McpToolAdapter` (`rupu-agent/src/mcp_tool.rs`) builds its descriptor from the MCP `ToolSpec`: `ToolKind::Read` → `Read`, `ToolKind::Write` → `External`, except `findings.record`/`findings.tag` → `Record`. W4 deletes the adapter. Until then this is the only place that maps `ToolKind` to `Effect`.

### 3.3 Canonical names and aliases (D11)

W1 sets the final catalog names, so later workstreams move tools without renaming them. **The model sees the canonical name.** An alias is accepted when it appears in `tools:` (W2 does the resolving) and when a model calls it, which happens when a user's agent prompt still says "call `report_finding`". Transcripts record the name the model actually used.

| Canonical | Aliases (accepted forever) | Effect | Needs |
|---|---|---|---|
| `bash` | — | Write | Netflow |
| `read_file` | — | Read | Coverage (optional emit) |
| `write_file`, `edit_file` | — | Write | — |
| `grep`, `glob`, `ast_grep` | — | Read | — |
| `coverage.mark` | `coverage_mark` | Record | Coverage |
| `coverage.status` | `coverage_status` | Read | Coverage |
| `coverage.remaining` | `coverage_remaining` | Read | Coverage |
| `coverage.concerns.search` | `coverage_concerns_search` | Read | Coverage |
| `coverage.concerns.detail` | `coverage_concerns_detail` | Read | Coverage |
| `findings.report` | `report_finding`, `findings.record` | Record | Findings |
| `findings.verify` | `finding.verify` | Record | Findings |
| `findings.query` | `query_findings` | Read | Findings |
| `findings.tag` | `tag_findings` | Record | Findings |
| `assets.mark` | `asset_mark` | Record | Findings |
| `scm.*` reads / `issues.{list,get,comments}` | — | Read | Scm |
| `scm.branches.create`, `scm.prs.{comment,create}`, `issues.{comment,create,update_state}`, `github.workflows_dispatch`, `gitlab.pipeline_trigger` | — | External | Scm |
| `board.claim`, `board.release`, `board.post`, `board.directive`, `board.retract` | — | Record | MessageBus |
| `board.read` | — | Read | MessageBus |
| `msg.send` | — | Record | MessageBus |
| `agents.list`, `agents.get`, `workflows.list`, `workflows.get`, `catalog.search` | — | Read | Catalog |
| `goal.status`, `budget.status` | — | Read | RunStatus |
| `goal.coverage` | `coverage.status`* | Read | RunStatus |
| `dispatch` | `dispatch_agent`, `dispatch_agents_parallel`, `run_workflow` (W7 shims) | Spawn | Launcher |
| `join` | — | Read | Launcher |
| `workflows.generate` | `generate_workflow` (W7 shim) | Spawn | WorkflowGenerator |

\* **Name collision, resolved by scope.** Agentiflow's `coverage.status` (goal coverage across the flow) is renamed `goal.coverage`. The canonical `coverage.status` becomes the ledger tool, formerly `coverage_status`. The old `coverage.status` alias resolves to `goal.coverage` **only for agents loaded inside an agentiflow lead** (the old name was only ever offered there). `GrantResolver` (W2) applies that rule; W1 records it as `AliasScope::FlowLead` on the alias.

The findings guidance appended to system prompts (`runner.rs:1793`) and the stock fleet's agent prompts (`crates/rupu-cli/templates/fleet/`) switch to canonical names in this card.

### 3.4 `PermissionPolicy` (`crates/rupu-tools/src/permission.rs`, rewritten)

```rust
pub enum PermissionMode { Readonly, Ask, Bypass }   // Ord: Readonly < Ask < Bypass
impl PermissionMode { pub fn parse(s: &str) -> Result<Self, UnknownMode>; } // the ONE parser

pub trait Prompter: Send + Sync {          // implemented by the CLI's TTY prompt
    fn ask(&self, req: &PromptRequest) -> PromptAnswer; // Allow | AllowAlways | Deny
}

pub struct PermissionPolicy { mode: PermissionMode, prompter: Option<Arc<dyn Prompter>> }

pub enum Decision {
    Allow,
    Deny { reason: DenyReason },
    /// Allowed; any child run this call starts is capped at `ceiling`.
    Spawn { ceiling: PermissionMode },
}

impl PermissionPolicy {
    /// `always` is the per-run set of tools the operator chose "allow always" for.
    pub fn decide(&self, d: &ToolDescriptor, input: &Value, always: &mut AllowAlways) -> Decision;
    /// The child ceiling rule (D7): min(this run's mode, child's own permissionMode).
    pub fn ceiling_for_child(&self, child_declared: Option<PermissionMode>) -> PermissionMode;
}
```

The decision table is the whole policy. It is tested exhaustively (§6).

| Effect ↓ / Mode → | `bypass` | `ask` + prompter | `ask`, no prompter | `readonly` |
|---|---|---|---|---|
| Read | allow | allow | allow | allow |
| Record | allow | allow | allow | allow (D4) |
| Write | allow | **prompt** | allow + `permission_mode_degraded` notice (once) | **deny** |
| External | allow | **prompt** (fixes T3) | allow + notice (once) | **deny** |
| Spawn | allow, ceiling = child's own mode | allow, ceiling = min(ask, child) | allow, ceiling = min(ask, child) | allow, **ceiling = readonly** (fixes R2) |

- **Prompt answers.** `Allow` covers this call. `AllowAlways` adds the canonical tool name to `always` (fixes T4: it no longer flips the mode). `Deny` returns `Deny { reason: OperatorDenied }`.
- **The degraded notice.** This is today's documented I-78 behaviour (ask means bypass in detached runs), now visible. The agent loop writes one `Notice { kind: "permission_mode_degraded", message: "ask mode has no operator in this run; write/external tools run without prompting" }` the first time it applies.

### 3.5 Agent loop wiring

- `AgentRunOpts.decider: Arc<dyn PermissionDecider>` is **replaced** by `permission: PermissionPolicy`. The `PermissionDecider` trait is deleted.
- In `runner.rs` around `:2913–3066`, per call: resolve the alias, read the descriptor, call `decide`, then branch. `Deny` writes the existing `permission_denied` result. `Spawn { ceiling }` passes the ceiling to the tool through `ToolContext.spawn_ceiling` (a new field; W3 folds it into `RunIdentity`).
- The five launch sites stop choosing a decider. They pass the mode they already compute (+ the CLI TTY `Prompter` for interactive `rupu run`). W3 then moves even that into `RunAssembler`.
- **Sub-agents (R2).** `CliAgentDispatcher::dispatch` takes the ceiling from the call's `ToolContext` and runs the child with `PermissionPolicy { mode: ceiling, prompter: parent's prompter }`. The child's `permissionMode` can only lower the ceiling. The interactive prompter is shared and serialized (a mutex around the TTY), so parallel children prompt one at a time.

### 3.6 Small fixes that land here

- **T16:** `rupu_tools::output::{ok, ok_json, failed, req_str, opt_str}` replaces the copies in agentiflow `tools.rs`/`dispatch_tools.rs`/`roster.rs` and `coverage_tools.rs` (`ok_output`/`err_output`). `attribution_from_ctx` (`coverage_tools.rs:26`) is replaced by `coverage_emit::attribution_from`.
- **T17:** `glob` filters results through `path_scope::is_inside`, as the other fs tools already do.
- `bash.rs`'s `RESERVED_NATIVE_TOOLS` (`:73`) lists the **non-core** tool names that a model wrongly tries to run as shell commands. Core names like `grep`/`glob` are real programs and must stay runnable. In W1 it stays a static list in `rupu-tools`, extended to every non-core canonical name **and** alias (findings/coverage/assets plus the agentiflow board/msg/goal/budget/roster names). Lockstep tests in `rupu-agent` and `rupu-agentiflow` (the crates that own those descriptors) assert that every non-core descriptor's names are in the list and that no core name is. W5/W7 replace the list with `ToolCatalog::non_shell_names()` once every descriptor lives in `rupu-tools`.

## 4. Files

| File | Change |
|---|---|
| `rupu-tools/src/descriptor.rs` | **new**: `Effect`, `Service`, `ToolDescriptor`, `AliasScope` |
| `rupu-tools/src/tool.rs` | `Tool` trait gains `descriptor()`. `spawn_ceiling` field on `ToolContext` |
| `rupu-tools/src/permission.rs` | rewritten: `PermissionMode::parse`, `PermissionPolicy`, `Prompter`, `Decision`, `AllowAlways`. `PermissionGate`/`KNOWN_*` **deleted** |
| `rupu-tools/src/output.rs` | **new**: shared output helpers |
| `rupu-tools/src/*.rs` (9 builtins), `rupu-agent/src/coverage_tools.rs` (10), `rupu-agent/src/mcp_tool.rs`, `rupu-agentiflow/src/{tools,status_tools,dispatch_tools,roster}.rs` (19) | add `static DESCRIPTOR`s with canonical names + aliases + effects |
| `rupu-agent/src/runner.rs` | the decision path above. Delete `BypassDecider`, `ReadonlyDecider`, `parse_mode_for_runtime`, `parse_mode_for_event` |
| `rupu-cli/src/cmd/run.rs` | `AskDecider` → `TtyPrompter: Prompter`. Duplicate `ReadonlyDecider` **deleted** |
| `rupu-cli/src/cmd/{session,dispatch}.rs`, `rupu-orchestrator/src/step_factory.rs`, `rupu-agentiflow/src/lead.rs` | pass mode (+ prompter) instead of a decider. dispatch.rs applies the ceiling |
| `rupu-mcp/src/permission.rs` | `McpPermission::check` delegates to `PermissionPolicy` using the spec's mapped effect (deleted in W4) |
| `rupu-cli/templates/fleet/**`, `.rupu/agents/**` | canonical tool names in prompts |
| `docs/` | permission modes section updated (effects table) |

## 5. Deleted

`PermissionDecider` trait and its 4 impls · `PermissionGate`, `KNOWN_READ_TOOLS`, `KNOWN_WRITE_TOOLS` · the duplicate `ReadonlyDecider` · `parse_mode_for_runtime` / `parse_mode_for_event` · per-crate `done`/`failed`/`req_str`/`ok_output`/`err_output` · `attribution_from_ctx` · every `== "readonly"` string comparison at launch sites.

## 6. Tests

All in each crate's single `tests/it/` binary.

1. **`permission_table`** (`rupu-tools`): every `Effect` × `PermissionMode` × {prompter answering Allow / AllowAlways / Deny, no prompter} gives the cell in §3.4. AllowAlways for tool A then a call to tool B prompts again (T4).
2. **`descriptors_complete`** (`rupu-tools`, extended in W4/W5): every tool in every registry builder has a descriptor; canonical names are unique; no alias equals another tool's canonical name or another alias (except the scoped `coverage.status`).
3. **`readonly_subagent_is_capped`** (`rupu-cli` serial or orchestrator it): a readonly parent dispatches a child whose frontmatter says `permissionMode: bypass`; the child's `write_file` is denied.
4. **`ask_prompts_for_mcp_write`** (`rupu-agent`): with a recording prompter, a call to an `External` MCP tool under ask reaches the prompter (T3).
5. **`degraded_notice_once`**: ask + no prompter + two `Write` calls → exactly one `permission_mode_degraded` notice.
6. **`alias_call_resolves`**: a model calling `report_finding` executes `findings.report`, and the transcript `ToolCall.tool == "report_finding"`.
7. **`glob_stays_in_workspace`**: a `../` pattern yields nothing outside the workspace.

## 7. Acceptance

- `grep -rn '"bash" | "write_file"' crates/` returns nothing. `grep -rn 'impl PermissionDecider' crates/` returns nothing.
- Readonly can't escape through children (test 3), and ask prompts for MCP writes (test 4).
- The stock fleet and `.rupu/agents` run unchanged apart from the canonical names in their prompts.
- The CLAUDE.md `rupu-tools` / `rupu-agent` entries are updated: descriptor + policy as the single permission path.
