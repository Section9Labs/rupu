# Subprocess netflow capture — Plan 1: record model + capture port

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the data-model and trait surface for subprocess network capture to `rupu-netflow`, with no OS code and no behavior change, so later plans (the `rupu-netwatch` backends, the bash wiring, the CP views) have stable types to build on.

**Architecture:** Pure additive changes to `rupu-netflow`: a new `Fidelity` level, a new `Origin` variant, new optional `FlowRecord`/`FlowCtx` fields, two new append-only `LedgerLine` variants, a provided `FlowSink` completion method, and a dependency-free `capture` port module (traits only). Every change is serde-additive — old ledgers read unchanged, old readers skip the new lines. Nothing here produces or consumes a socket flow yet.

**Tech Stack:** Rust 2021, `serde`, `async-trait`, `chrono`, `ulid`. Tests use the single `tests/it` binary (`crates/rupu-netflow/tests/it/main.rs`).

**Spec:** `docs/superpowers/specs/2026-10-04-rupu-subprocess-netflow-capture-design.md` (this plan is §17 item 1; the model is §6–§7).

## Global Constraints

- **No `unsafe`.** Workspace sets `unsafe_code = "forbid"`. Nothing in this plan needs FFI; if a later plan hits that wall it stops and asks (spec §18 V2).
- **`#![deny(clippy::all)]` workspace-wide**, `unsafe_code` forbidden, workspace dependency versions only (never pin in a crate `Cargo.toml`).
- **Integration tests: one binary per crate.** Add modules under `crates/rupu-netflow/tests/it/`, listed in `main.rs`; never a new top-level `tests/*.rs`.
- **Additive only.** No existing serialized shape may change meaning. Old ledgers must still parse; the existing HTTP `FlowSink::complete` path must be untouched.
- **Fidelity honesty.** Least-observable ordering is `Coarse < Socket < Http < Full` everywhere it is compared.
- **Direction on Linux is blank for v1** (spec §18 V1): the `Direction` type exists, but this plan does not commit any backend to populating it.

---
## File structure

| File | Responsibility | Change |
|---|---|---|
| `crates/rupu-netflow/src/record.rs` | `Fidelity`, `Outcome`, `FlowRecord`, `LedgerLine`, new `FlowProcess`/`Direction` | Modify |
| `crates/rupu-netflow/src/ctx.rs` | `FlowCtx` (+ `tool_call_id`), `Origin` (+ `Subprocess`) | Modify |
| `crates/rupu-netflow/src/ledger/views.rs` | fold new `SocketComplete` into its flow; ignore `Capture` | Modify |
| `crates/rupu-netflow/src/ledger/explorer.rs` | `origin_key` for `Subprocess`; fidelity ordering includes `Socket` | Modify |
| `crates/rupu-netflow/src/sink.rs` | provided `complete_socket`; impls on `FanoutSink`/`MemorySink`/`NullSink` | Modify |
| `crates/rupu-netflow/src/ledger/writer.rs` | `NetflowWriter::complete_socket` offers a `SocketComplete` line | Modify |
| `crates/rupu-netflow/src/capture.rs` | **new** — `SubprocessCapture`/`CaptureCall` traits, `CallAttribution`, `CaptureState` | Create |
| `crates/rupu-netflow/src/lib.rs` | module decl + re-exports | Modify |
| `crates/rupu-netflow/tests/it/capture.rs` | existing module — add model + port tests | Modify |

Deliberate refinement of spec §7.6: instead of mutating the existing `LedgerLine::Complete` (which would churn the HTTP completion path at `writer.rs:80` and `views.rs:49`), this plan adds a separate `LedgerLine::SocketComplete`. Same observable result, strictly additive, HTTP path untouched. If matt prefers the in-place mutation, this is the one decision to flip before starting.

---
### Task 1: `Fidelity::Socket`

**Files:**
- Modify: `crates/rupu-netflow/src/record.rs` (the `Fidelity` enum, ~line 16)
- Modify: `crates/rupu-netflow/src/ledger/explorer.rs:582-586` (the ordering match)
- Test: `crates/rupu-netflow/tests/it/capture.rs`

**Interfaces:**
- Produces: `Fidelity::Socket` (serde tag `"socket"`); ordering rank `Coarse=0 < Socket=1 < Http=2 < Full=3`.

- [ ] **Step 1: Write the failing test**

In `crates/rupu-netflow/tests/it/capture.rs`:

```rust
use rupu_netflow::Fidelity;

#[test]
fn socket_fidelity_serializes_snake_case_and_round_trips() {
    let json = serde_json::to_string(&Fidelity::Socket).unwrap();
    assert_eq!(json, r#""socket""#);
    assert_eq!(serde_json::from_str::<Fidelity>(&json).unwrap(), Fidelity::Socket);
}
```

- [ ] **Step 2: Run it, confirm it fails**

Run: `cargo test -p rupu-netflow --test it capture::socket_fidelity`
Expected: FAIL — no variant `Socket`.

- [ ] **Step 3: Add the variant**

In `record.rs`, add to `Fidelity` between `Coarse` and `Http`:

```rust
    /// Connection-level: process, remote IP:port, bytes and timing
    /// observed from the OS socket table (spec 2026-10-04). No URL,
    /// method or HTTP status. More observable than `Coarse`, less than
    /// `Http`.
    Socket,
```

- [ ] **Step 4: Extend the ordering in `explorer.rs`**

The match at `explorer.rs:582-586` currently maps `Coarse=>0, Http=>1, Full=>2`. Replace with:

```rust
                    Fidelity::Coarse => 0u8,
                    Fidelity::Socket => 1,
                    Fidelity::Http => 2,
                    Fidelity::Full => 3,
```

- [ ] **Step 5: Run the test + the explorer tests**

Run: `cargo test -p rupu-netflow --test it capture::socket_fidelity`
Run: `cargo test -p rupu-netflow --test it explorer`
Expected: PASS (the `coarse_first` explorer tests still hold — `Coarse` is still lowest).

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-netflow/src/record.rs crates/rupu-netflow/src/ledger/explorer.rs crates/rupu-netflow/tests/it/capture.rs
git commit -m "netflow: add Fidelity::Socket level"
```

---
### Task 2: `Origin::Subprocess(String)`

**Files:**
- Modify: `crates/rupu-netflow/src/ctx.rs` (the `Origin` enum + its doc comment + the `origin_enumerates_only_egress_that_can_occur` test, ~line 86 onward)
- Modify: `crates/rupu-netflow/src/ledger/explorer.rs:115-123` (`origin_key`)
- Test: `crates/rupu-netflow/tests/it/capture.rs`

**Interfaces:**
- Produces: `Origin::Subprocess(String)`, adjacently tagged (`{"kind":"subprocess","name":"curl"}`); `origin_key(&Origin::Subprocess("curl".into())) == "subprocess:curl"`.

- [ ] **Step 1: Write the failing test**

```rust
use rupu_netflow::Origin;
use rupu_netflow::ledger::explorer::origin_key; // re-exported; see Step 4

#[test]
fn subprocess_origin_tags_and_keys() {
    let o = Origin::Subprocess("curl".into());
    let json = serde_json::to_value(&o).unwrap();
    assert_eq!(json, serde_json::json!({"kind":"subprocess","name":"curl"}));
    assert_eq!(serde_json::from_value::<Origin>(json).unwrap(), o);
    assert_eq!(origin_key(&o), "subprocess:curl");
}
```

- [ ] **Step 2: Run it, confirm it fails**

Run: `cargo test -p rupu-netflow --test it capture::subprocess_origin`
Expected: FAIL — no variant `Subprocess` (and possibly `origin_key` not re-exported).

- [ ] **Step 3: Add the variant**

In `ctx.rs`, add to `Origin` (after `Scm`):

```rust
    /// A process the agent started through the `bash` tool, by process
    /// name (`curl`, `git`, `nmap`…), or `"unknown"` when the owning
    /// process could not be resolved. Flows tagged this way are
    /// `Fidelity::Socket` and carry `FlowCtx.tool_call_id`.
    Subprocess(String),
```

- [ ] **Step 4: Update `origin_key` and re-export it**

In `explorer.rs`, add the arm to `origin_key`:

```rust
        Origin::Subprocess(name) => format!("subprocess:{name}"),
```

Confirm `pub fn origin_key` is reachable from tests; if `explorer` is not already re-exported, add to `crates/rupu-netflow/src/ledger/mod.rs` the line `pub use explorer::origin_key;` and to `lib.rs` nothing further (tests use `rupu_netflow::ledger::explorer::origin_key`). If `explorer` is already `pub mod`, no change is needed.

- [ ] **Step 5: Update the `Origin` doc comment and the enumeration test**

In `ctx.rs`, the `Origin` doc comment explains why `Mcp`/`Webhook` are absent. Add a sentence: `Subprocess` IS present and CAN occur — it is emitted by the subprocess-capture backend (spec 2026-10-04). In the `origin_enumerates_only_egress_that_can_occur` test, add a line asserting `{"kind":"subprocess","name":"curl"}` parses.

- [ ] **Step 6: Run the tests**

Run: `cargo test -p rupu-netflow --test it capture::subprocess_origin`
Run: `cargo test -p rupu-netflow origin`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-netflow/src/ctx.rs crates/rupu-netflow/src/ledger/explorer.rs crates/rupu-netflow/src/ledger/mod.rs crates/rupu-netflow/tests/it/capture.rs
git commit -m "netflow: add Origin::Subprocess for bash-tool connections"
```

---
### Task 3: `FlowCtx.tool_call_id`

**Files:**
- Modify: `crates/rupu-netflow/src/ctx.rs` (the `FlowCtx` struct, ~line 63)
- Test: `crates/rupu-netflow/tests/it/capture.rs`

**Interfaces:**
- Produces: `FlowCtx.tool_call_id: Option<String>`, `#[serde(default, skip_serializing_if = "Option::is_none")]`. `FlowCtx::system(..)` leaves it `None`.

- [ ] **Step 1: Write the failing test**

```rust
use rupu_netflow::{FlowCtx, Origin};

#[test]
fn flow_ctx_tool_call_id_is_optional_and_omitted_when_none() {
    let mut c = FlowCtx::system(Origin::Subprocess("curl".into()));
    assert!(serde_json::to_string(&c).unwrap().find("tool_call_id").is_none());
    c.tool_call_id = Some("toolu_01Ab".into());
    let json = serde_json::to_string(&c).unwrap();
    assert!(json.contains(r#""tool_call_id":"toolu_01Ab""#));
    assert_eq!(serde_json::from_str::<FlowCtx>(&json).unwrap(), c);
}
```

- [ ] **Step 2: Run it, confirm it fails**

Run: `cargo test -p rupu-netflow --test it capture::flow_ctx_tool_call_id`
Expected: FAIL — no field `tool_call_id`.

- [ ] **Step 3: Add the field**

In `FlowCtx` (after `workspace_id`):

```rust
    /// The `bash` tool call that caused this flow. Set only on
    /// `Fidelity::Socket` flows; `None` for every HTTP flow and for
    /// `FlowCtx::system`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
```

`FlowCtx::system` sets every other field already; add `tool_call_id: None` to its literal.

- [ ] **Step 4: Fix any struct-literal construction sites**

Run `cargo build -p rupu-netflow` and add `tool_call_id: None` to any `FlowCtx { .. }` literal the compiler flags (notably the `sample()` test in `record.rs` and `ctx.rs` tests). Struct-update (`..`) sites need no change.

- [ ] **Step 5: Run the test + crate build**

Run: `cargo test -p rupu-netflow --test it capture::flow_ctx_tool_call_id`
Run: `cargo build -p rupu-netflow`
Expected: PASS, clean build.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-netflow/src/ctx.rs crates/rupu-netflow/src/record.rs crates/rupu-netflow/tests/it/capture.rs
git commit -m "netflow: add FlowCtx.tool_call_id for socket-flow attribution"
```

---

### Task 4: `FlowProcess`, `Direction`, and the new `FlowRecord` fields

**Files:**
- Modify: `crates/rupu-netflow/src/record.rs` (new types + three `FlowRecord` fields)
- Test: `crates/rupu-netflow/tests/it/capture.rs`

**Interfaces:**
- Produces:
  - `pub struct FlowProcess { pub pid: u32, pub name: String }` (`Debug, Clone, PartialEq, Eq, Serialize, Deserialize`)
  - `pub enum Direction { Outbound, Inbound }` (`serde(rename_all="snake_case")`, same derives)
  - `FlowRecord.process: Option<FlowProcess>`, `FlowRecord.local_addr: Option<String>`, `FlowRecord.direction: Option<Direction>` — all `#[serde(default, skip_serializing_if = "Option::is_none")]`.

- [ ] **Step 1: Write the failing test**

```rust
use rupu_netflow::{Direction, FlowProcess};

#[test]
fn flow_process_and_direction_round_trip() {
    let p = FlowProcess { pid: 4412, name: "curl".into() };
    assert_eq!(serde_json::from_str::<FlowProcess>(&serde_json::to_string(&p).unwrap()).unwrap(), p);
    let d = Direction::Outbound;
    assert_eq!(serde_json::to_string(&d).unwrap(), r#""outbound""#);
    assert_eq!(serde_json::from_str::<Direction>(r#""inbound""#).unwrap(), Direction::Inbound);
}
```

- [ ] **Step 2: Run it, confirm it fails**

Run: `cargo test -p rupu-netflow --test it capture::flow_process_and_direction`
Expected: FAIL — unknown types.

- [ ] **Step 3: Add the types and fields**

In `record.rs`, above `FlowRecord`:

```rust
/// The process that owned a captured socket (socket flows only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlowProcess {
    pub pid: u32,
    pub name: String,
}

/// Connection direction for socket flows. `None` on a record means
/// unknown — e.g. Linux v1 does not populate it (spec 2026-10-04 18 V1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Outbound,
    Inbound,
}
```

In `FlowRecord`, after `resolved_ips`:

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<FlowProcess>,
    /// `"ip:port"` of the local end, for correlating with other logs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_addr: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<Direction>,
```

- [ ] **Step 4: Fix construction sites**

Run `cargo build -p rupu-netflow`; the `sample()` in `record.rs` and any `FlowRecord { .. }` literal (e.g. `netflow_sink.rs` tests, `explorer.rs` test helpers, `rupu-cli/src/netflow_sink.rs` test) need `process: None, local_addr: None, direction: None`. Because they are `skip_serializing_if`, existing stored ledgers and the HTTP write path are unaffected.

- [ ] **Step 5: Run the test + full crate build**

Run: `cargo test -p rupu-netflow --test it capture::flow_process_and_direction`
Run: `cargo build -p rupu-netflow`
Expected: PASS, clean build.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-netflow/src/record.rs crates/rupu-netflow/tests/it/capture.rs
git commit -m "netflow: add process/local_addr/direction fields for socket flows"
```

---
### Task 5: `LedgerLine::SocketComplete` + `FlowSink::complete_socket`

**Files:**
- Modify: `crates/rupu-netflow/src/record.rs` (new `LedgerLine` variant + a `SocketCompletion` struct)
- Modify: `crates/rupu-netflow/src/ledger/views.rs:44-67` (fold)
- Modify: `crates/rupu-netflow/src/sink.rs` (provided method + three impls)
- Modify: `crates/rupu-netflow/src/ledger/writer.rs:79-86` (offer the line)
- Test: `crates/rupu-netflow/tests/it/capture.rs`

**Interfaces:**
- Produces:
  - `pub struct SocketCompletion { pub id: FlowId, pub duration_ms: u64, pub bytes_in: Option<u64>, pub bytes_out: Option<u64>, pub outcome: Option<Outcome>, pub error: Option<String> }` (Clone, Debug, PartialEq, Eq)
  - `LedgerLine::SocketComplete(SocketCompletion)` (serde tag `"socket_complete"`; the inner struct flattened via `#[serde(flatten)]` or boxed if clippy's `large_enum_variant` complains — see Step 3)
  - `FlowSink::complete_socket(&self, c: SocketCompletion)` — a **provided** method defaulting to no-op, so no existing impl breaks.
  - `NetflowWriter::complete_socket` offers a `SocketComplete` line; `read_flows*` folds it into the referenced flow (sets `bytes_in`/`bytes_out`/`duration_ms`/`outcome`/`error` where `Some`, and `body_complete = true`).

- [ ] **Step 1: Write the failing test**

```rust
use rupu_netflow::{Fidelity, FlowId, FlowRecord, LedgerLine, Outcome, SocketCompletion};
use rupu_netflow::ledger::views::read_flows;

#[test]
fn socket_complete_round_trips_and_folds_into_its_flow() {
    let c = SocketCompletion {
        id: FlowId::from_parts(5, 5),
        duration_ms: 553,
        bytes_in: Some(48000),
        bytes_out: Some(1200),
        outcome: Some(Outcome::Ok),
        error: None,
    };
    let line = LedgerLine::SocketComplete(c.clone());
    let json = serde_json::to_string(&line).unwrap();
    assert!(json.contains(r#""type":"socket_complete""#));
    assert_eq!(serde_json::from_str::<LedgerLine>(&json).unwrap(), line);
}
```

Plus a fold test that writes a `Flow` (socket fidelity) then a `SocketComplete` for the same id to a temp ledger and asserts `read_flows` returns one flow with `body_complete == true`, `bytes_in == Some(48000)`, `duration_ms == Some(553)`. Use `tempfile::NamedTempFile` and `serde_json::to_writer` + newline per line, matching the pattern in `views.rs` tests (`read_flows_folds_complete_into_its_flow`, ~line 411).

- [ ] **Step 2: Run it, confirm it fails**

Run: `cargo test -p rupu-netflow --test it capture::socket_complete`
Expected: FAIL — unknown `SocketCompletion` / `LedgerLine::SocketComplete`.

- [ ] **Step 3: Add the struct and variant**

In `record.rs`, add `SocketCompletion` (above `LedgerLine`), then a variant:

```rust
    /// Finalizes a socket flow at close: byte counts, final outcome and
    /// an optional note. Separate from `Complete` (the HTTP streamed-body
    /// finalizer) so the HTTP path is untouched. Any `Some` field
    /// overwrites the flow; `None` leaves it as written.
    SocketComplete(SocketCompletion),
```

If clippy's `large_enum_variant` fires (the struct is small, so it likely will not), box it: `SocketComplete(Box<SocketCompletion>)` and adjust the fold/test. Run `cargo clippy -p rupu-netflow` to decide.

- [ ] **Step 4: Fold it in `views.rs`**

In `read_flows_and_dropped`'s match (after the `Complete` arm, ~line 60):

```rust
            LedgerLine::SocketComplete(c) => {
                if let Some(&i) = index.get(&c.id) {
                    if let Some(b) = c.bytes_in { flows[i].bytes_in = Some(b); }
                    if let Some(b) = c.bytes_out { flows[i].bytes_out = Some(b); }
                    if let Some(o) = c.outcome { flows[i].outcome = o; }
                    if c.error.is_some() { flows[i].error = c.error; }
                    flows[i].duration_ms = Some(c.duration_ms);
                    flows[i].body_complete = true;
                }
            }
```

(If `read_flows_in_range` has its own match arm near line 424, add the same arm there.)

- [ ] **Step 5: Add the sink method + writer offer**

In `sink.rs`, add to the `FlowSink` trait a provided method:

```rust
    /// Finalize a socket flow. Default no-op: sinks that don't persist
    /// completions (e.g. the transcript bridge) ignore it.
    async fn complete_socket(&self, _c: crate::record::SocketCompletion) {}
```

Implement it on `FanoutSink` (fan to children, mirroring `complete`) and `MemorySink` (push into a new `socket_completions` vec with an accessor). `NullSink` and `TranscriptSink` inherit the default. In `writer.rs`, add:

```rust
    async fn complete_socket(&self, c: crate::record::SocketCompletion) {
        self.offer(LedgerLine::SocketComplete(c));
    }
```

- [ ] **Step 6: Run tests + clippy**

Run: `cargo test -p rupu-netflow --test it capture::socket_complete`
Run: `cargo test -p rupu-netflow`
Run: `cargo clippy -p rupu-netflow`
Expected: PASS, no clippy findings.

- [ ] **Step 7: Commit**

```bash
git add crates/rupu-netflow/src
git commit -m "netflow: add SocketComplete ledger line and FlowSink::complete_socket"
```

---
### Task 6: `LedgerLine::Capture` (capture-state / visible-loss line)

**Files:**
- Modify: `crates/rupu-netflow/src/record.rs` (new `LedgerLine::Capture` variant + `CaptureState` enum)
- Modify: `crates/rupu-netflow/src/ledger/views.rs` (fold: ignore it — must not disturb flows/dropped)
- Test: `crates/rupu-netflow/tests/it/capture.rs`

**Interfaces:**
- Produces:
  - `pub enum CaptureState { Active { backend: String }, Unavailable { reason: String } }` (serde tag `"state"`, `rename_all="snake_case"`, derives Clone/Debug/PartialEq/Eq)
  - `LedgerLine::Capture { ts: DateTime<Utc>, state: CaptureState, tool_call_id: Option<String>, note: Option<String> }` (serde tag `"capture"`)
  - `read_flows*` ignores `Capture` lines (flows and dropped counts unchanged), and a reader built before this variant existed skips it as a malformed/unknown line without failing.

- [ ] **Step 1: Write the failing test**

```rust
use chrono::Utc;
use rupu_netflow::{CaptureState, LedgerLine};

#[test]
fn capture_line_round_trips_and_is_ignored_by_read_flows() {
    let line = LedgerLine::Capture {
        ts: Utc::now(),
        state: CaptureState::Unavailable { reason: "cgroup v2 not mounted".into() },
        tool_call_id: Some("toolu_01Ab".into()),
        note: None,
    };
    let json = serde_json::to_string(&line).unwrap();
    assert!(json.contains(r#""type":"capture""#));
    assert_eq!(serde_json::from_str::<LedgerLine>(&json).unwrap(), line);
}
```

Plus a fold test: write one `Flow`, one `Capture`, one `Dropped { count: 2 }` to a temp ledger; assert `read_flows_and_dropped` returns exactly one flow and `dropped == 2` (the `Capture` line neither adds a flow nor affects the count).

- [ ] **Step 2: Run it, confirm it fails**

Run: `cargo test -p rupu-netflow --test it capture::capture_line`
Expected: FAIL — unknown `CaptureState` / variant.

- [ ] **Step 3: Add the enum and variant**

```rust
/// What the subprocess-capture backend could do for a run. Written as a
/// `LedgerLine::Capture` so the CP can surface "capture unavailable:
/// <reason>" honestly (spec 2026-10-04 13).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CaptureState {
    Active { backend: String },
    Unavailable { reason: String },
}
```

Variant on `LedgerLine`:

```rust
    /// Capture availability for a run, plus any visible-loss note.
    Capture {
        ts: DateTime<Utc>,
        state: CaptureState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tool_call_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
```

- [ ] **Step 4: Ignore it in the fold**

In `read_flows_and_dropped` (and `read_flows_in_range` if it matches variants), add:

```rust
            LedgerLine::Capture { .. } => {}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rupu-netflow --test it capture::capture_line`
Run: `cargo test -p rupu-netflow`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-netflow/src crates/rupu-netflow/tests/it/capture.rs
git commit -m "netflow: add Capture ledger line for capture-state and visible loss"
```

---
### Task 7: The `capture` port module (traits only)

**Files:**
- Create: `crates/rupu-netflow/src/capture.rs`
- Modify: `crates/rupu-netflow/src/lib.rs` (module decl + re-exports)
- Test: `crates/rupu-netflow/tests/it/capture.rs`

**Interfaces:**
- Produces (exact signatures later plans build on):
  - `pub struct CallAttribution { pub run_id: String, pub step_id: Option<String>, pub agent: Option<String>, pub codename: Option<String>, pub tool_call_id: String, pub sink: std::sync::Arc<dyn crate::sink::FlowSink> }`
  - `pub trait SubprocessCapture: Send + Sync { fn begin(&self, call: CallAttribution) -> Box<dyn CaptureCall>; fn run_finished(&self, run_id: &str); }`
  - `pub trait CaptureCall: Send { fn shell_prefix(&self) -> Option<String>; fn spawned(&mut self, pid: u32); fn finished(self: Box<Self>); }`
  - `pub struct NoopCapture;` implementing `SubprocessCapture` — `begin` returns a `NoopCall` whose `shell_prefix` is `None` and whose `spawned`/`finished` do nothing; `run_finished` does nothing. This is the value the unsupported-OS path and "capture disabled" config return, and it is what Plan 5's bash wiring can default to in tests.

These traits are NOT `async`: `begin`/`finished` must be cheap and synchronous (spec §6). Heavy work belongs on the backend's own thread (Plan 2+).

- [ ] **Step 1: Write the failing test**

```rust
use std::sync::Arc;
use rupu_netflow::capture::{CallAttribution, NoopCapture, SubprocessCapture};
use rupu_netflow::NullSink;

#[test]
fn noop_capture_is_inert() {
    let cap = NoopCapture;
    let call = CallAttribution {
        run_id: "run-1".into(),
        step_id: None,
        agent: Some("recon".into()),
        codename: None,
        tool_call_id: "toolu_01Ab".into(),
        sink: Arc::new(NullSink),
    };
    let mut c = cap.begin(call);
    assert_eq!(c.shell_prefix(), None);
    c.spawned(4412);
    Box::new(c).finished(); // compiles: finished consumes the box
    cap.run_finished("run-1");
}
```

Note: `begin` returns `Box<dyn CaptureCall>`; adjust the test so `finished` is called on the boxed value (`c.finished()` where `c: Box<dyn CaptureCall>` — `finished(self: Box<Self>)` is callable directly on the box). Simplify to:

```rust
    let c = cap.begin(call);
    assert_eq!(c.shell_prefix(), None);
    // spawned needs &mut; rebind through the box
    let mut c = c;
    c.spawned(4412);
    c.finished();
```

- [ ] **Step 2: Run it, confirm it fails**

Run: `cargo test -p rupu-netflow --test it capture::noop_capture`
Expected: FAIL — module `capture` does not exist.

- [ ] **Step 3: Write `capture.rs`**

Create `crates/rupu-netflow/src/capture.rs` with the `CallAttribution` struct, the two traits (exact signatures above), and:

```rust
/// The capture a disabled or unsupported backend hands back: inert.
pub struct NoopCapture;

struct NoopCall;

impl SubprocessCapture for NoopCapture {
    fn begin(&self, _call: CallAttribution) -> Box<dyn CaptureCall> { Box::new(NoopCall) }
    fn run_finished(&self, _run_id: &str) {}
}

impl CaptureCall for NoopCall {
    fn shell_prefix(&self) -> Option<String> { None }
    fn spawned(&mut self, _pid: u32) {}
    fn finished(self: Box<Self>) {}
}
```

Document on `SubprocessCapture` that `begin`/`finished` are synchronous and cheap, and that implementations must never let a capture failure affect the bash call (spec §13).

- [ ] **Step 4: Declare + re-export in `lib.rs`**

Add `pub mod capture;` and to the re-export block:

```rust
pub use capture::{CallAttribution, CaptureCall, NoopCapture, SubprocessCapture};
```

- [ ] **Step 5: Run the test + crate build + clippy**

Run: `cargo test -p rupu-netflow --test it capture::noop_capture`
Run: `cargo clippy -p rupu-netflow`
Expected: PASS, no findings.

- [ ] **Step 6: Commit**

```bash
git add crates/rupu-netflow/src/capture.rs crates/rupu-netflow/src/lib.rs crates/rupu-netflow/tests/it/capture.rs
git commit -m "netflow: add the SubprocessCapture port (traits + NoopCapture)"
```

---

## Final verification

- [ ] **Whole-crate gate**

Run: `cargo test -p rupu-netflow`
Run: `cargo clippy -p rupu-netflow --all-targets`
Run: `cargo build -p rupu-cli -p rupu-cp -p rupu-transcript` (the crates that construct `FlowRecord`/`FlowCtx` literals — confirms the additive fields broke no downstream literal).
Expected: all green. If a downstream literal fails to build, add the three `None` fields / `tool_call_id: None` there; it is a construction site the additive change surfaced, not a design problem.

- [ ] **Confirm no behavior change**

Grep the diff: the only runtime-reachable new code is the `views.rs` fold arms (which only fire on the new line types, never emitted in this plan) and the `NoopCapture` (inert). No existing call site emits a socket flow, a `SocketComplete`, or a `Capture` line yet — that is Plan 5. Confirm `git grep -n "Fidelity::Socket\|Origin::Subprocess\|SocketComplete\|LedgerLine::Capture\|complete_socket" crates/rupu-cli crates/rupu-cp crates/rupu-agent crates/rupu-tools` returns nothing (no premature wiring).

## Notes for later plans

- **Plan 2** implements `SubprocessCapture` in the new `rupu-netwatch` crate (pure `tracker.rs` + `unsupported.rs`) and `rupu-runtime::net_capture::shared`. The traits and `NoopCapture` from Task 7 are its contract.
- **Plan 5** adds the `ToolContext` fields and emits the first real `Flow`/`SocketComplete`/`Capture` lines from the bash tool; the choke-point-style wiring test lives there.
- **`MemorySink::socket_completions`** (Task 5) is the handle Plan 5's bash tests assert on.
