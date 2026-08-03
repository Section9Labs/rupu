# Netflow observability — egress capture, ledger, and CP topology view (design)

**Date:** 2026-08-03
**Status:** Design — approved by matt (capture scope, map semantics, telemetry-only stance locked in §2). Ready to plan.
**Scope:** new `crates/rupu-netflow` (types, ports, ledger, ASN, HTTP capture); `crates/rupu-transcript` (`Event::NetFlow`); `crates/rupu-config` (`NetflowConfig`); the seven crates that construct HTTP clients (`rupu-providers`, `rupu-scm`, `rupu-auth`, `rupu-update`, `rupu-webhook`, `rupu-cp`, `rupu-cli`); `crates/rupu-cp` (`api/netflow.rs` + `web/src/components/netflow/`).
**Out of scope (this arc):** the microVM capture backend (§9 — a later arc, designed-for but not built); egress *enforcement* / deny-by-default policy; findings integration (explicitly rejected — §2.5); TLS interception.

## 1. The problem

rupu has rich observability for what an agent *does* — `ToolCall` / `ToolResult` / `CommandRun` / `ToolAudit` in the transcript, coverage ledgers, findings, usage accounting. It has **nothing** for where an agent *reaches*.

Verified against the code:

- `BashTool` (`crates/rupu-tools/src/bash.rs`) locks cwd to the workspace and clears the environment down to an allowlist, but the subprocess has **unrestricted network access**. Nothing records that `cargo build` contacted `crates.io`, or that an `npm install` postinstall script phoned home.
- rupu's own outbound traffic — provider APIs, SCM connectors, MCP, webhooks, the update checker — is equally unrecorded.
- There is **no HTTP choke point**. Roughly 30 `reqwest::Client::new()` / `Client::builder()` sites are scattered across seven crates. One dependency (`octocrab`, in `crates/rupu-scm/src/connectors/github/client.rs`) brings its own `hyper`/`tower` stack. The workspace `gitlab = "0.1710"` pin is **not** used by `rupu-scm` — the GitLab, Jira and Linear connectors are hand-rolled `reqwest`, so they migrate cleanly.

So the first task is not "add capture" — it is "create the choke point that does not exist, then keep it the only door."

## 2. Decisions (operator-locked)

### 2.1 Goal: forensics, phased honestly

The end state is full-fidelity forensics: every flow, any port, any IP, any protocol, direct or indirect. **This arc does not deliver that.** It delivers rupu's own egress, plus the schema, ledger and views that a later full-fidelity backend fills in without re-plumbing anything.

### 2.2 Capture scope: rupu's own egress only

Phase 1 instruments rupu's own HTTP clients. It explicitly does **not** cover anything the agent's `bash` subprocesses do. That limitation is stated in the UI (§2.4), not glossed.

### 2.3 The microVM is the eventual full-fidelity backend — deferred

Evaluated and accepted as the right long-term answer, because it is the only mechanism that behaves identically on macOS and Linux:

- **macOS:** `VZFileHandleNetworkDeviceAttachment` (Virtualization.framework) hands the guest's virtio-net frames directly to rupu's process as raw Ethernet over a datagram socket — rupu becomes the virtual switch. No root. `com.apple.security.virtualization` is **self-signable**, so unlike the `NetworkExtension` content-filter path it needs no Apple-issued entitlement.
- **Linux:** KVM (Firecracker / cloud-hypervisor / libkrun) with a userspace net backend (passt, or libkrun's TSI which surfaces guest `connect()` as socket-level events).

Rejected mechanisms and why: `EndpointSecurity` has no connect event; `NetworkExtension` needs an Apple-approved entitlement plus a signed, notarized system extension (rupu is not yet notarized); eBPF is Linux-only and needs root; `pcap` gives no PID attribution; socket-table polling misses short-lived flows.

The real cost of the microVM is **not** the VM — it is the guest image supply chain (toolchains, virtiofs workspace share, credential injection). That is execution-model work, not observability work, and it gets its own arc. See §9.

### 2.4 "Map" means topology + ASN, not geography

The primary visualization is a **bipartite topology graph**, not a world map. A geographic map was considered and rejected: nearly every endpoint resolves to a CDN edge, so a country pin shows a PoP rather than who was actually contacted — visually striking, forensically false.

ASN/org enrichment (`AS13335 Cloudflare`) is the honest form of "where did this go" and is included.

### 2.5 Pure telemetry — no findings coupling

Anomalous egress does **not** become a `Finding`. Netflow carries no severity, no triage state, and no coupling to the findings subsystem. Should anomaly surfacing be wanted later, it is an additive detector on top of this data, not a change to it.

### 2.6 ASN data is acquired and refreshed automatically

The operator never has to run a command to get ASN enrichment. See §6.

## 3. Architecture — `crates/rupu-netflow`

A new lib crate defining the `FlowSink` port plus its adapters, per the workspace hexagonal rule. Capture backends are discussed below.

### 3.1 Ports

```rust
/// Where flow records go.
#[async_trait]
pub trait FlowSink: Send + Sync {
    async fn record(&self, flow: FlowRecord);
    async fn complete(&self, id: FlowId, bytes_in: u64, duration_ms: u64);
}
```

**On the second port.** The phase-1 capture adapter is a `reqwest` middleware that *pushes* into a sink from the request path. A `FlowSource` trait to abstract capture backends is deliberately **not** built in this arc: with a single push-based implementation it would be an indirection with nothing on the other side of it, and a trait whose only impl is a no-op is exactly the kind of speculative scaffolding this codebase avoids. It lands together with the microVM backend, which is the first implementation that genuinely needs it (a pull-based loop reading guest frames).

What actually guarantees the microVM "slots in without re-plumbing" is not a trait — it is the stability of `FlowRecord`, the ledger line format, and the CP API contract. Those are fixed by this arc.

`FlowSink` adapters compose via a `FanoutSink`:

| Adapter | Destination |
|---|---|
| `LedgerSink` | `.rupu/netflow/` append-only JSONL (§5) |
| `TranscriptSink` | bridges to `rupu_transcript::Event::NetFlow` |
| `BroadcastSink` | in-memory broadcast for live subscribers |
| `NullSink` | capture disabled |

### 3.2 Feature split

The crate must not drag `reqwest` into a schema crate:

```toml
[features]
default = []          # types + ledger + ASN only
http = ["dep:reqwest", "dep:reqwest-middleware"]
```

`rupu-transcript` depends with `default-features = false`, so `Event::NetFlow { flow: FlowRecord }` reuses the struct without inheriting an HTTP stack. `rupu-netflow` never depends on `rupu-transcript` — the bridge lives in the sink adapter, owned by the consumer that owns the transcript writer.

### 3.3 Module layout

```
crates/rupu-netflow/src/
  lib.rs
  record.rs      FlowRecord, FlowId, Origin, Outcome, Fidelity
  ctx.rs         FlowCtx
  sink.rs        FlowSink trait, FanoutSink, NullSink
  ledger/
    mod.rs
    paths.rs     NetflowPaths (mirrors rupu-coverage's shape)
    writer.rs    NetflowWriter / NetflowWriterHandle (bounded channel + bg task)
    views.rs     read_flows, host_rollup, graph_view
  asn/
    mod.rs       AsnTable lookup
    acquire.rs   fetch + compact + freshness check
  http/          (feature = "http")
    mod.rs       client(ctx) factory
    middleware.rs
    resolver.rs  custom dns_resolver capturing all A/AAAA answers
```

## 4. Attribution — bound at construction, not at request

The conventional approaches both fail here. `tokio::task_local!` does not propagate into `tokio::spawn`; `tracing` span context has the same hole and additionally requires every intermediate future to be `.instrument()`ed. Either would produce flows with silently missing attribution, which is worse than none.

Instead, **the context is bound when the client is built**. Provider and SCM clients are already constructed per-run, so this costs no new plumbing:

```rust
let client = rupu_netflow::client(FlowCtx {
    run_id:       Some(run_id),
    step_id:      Some(step_id),
    agent:        Some(agent_name),
    workspace_id,
    origin:       Origin::Provider("anthropic"),
});
```

```rust
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum Origin {
    Provider(String),   // anthropic, openai, gemini, copilot, codex, local…
    Scm(String),        // github, gitlab, jira, linear
    Mcp(String),        // server name
    Webhook,
    Update,
    Cp,
    System,
}
```

`String` rather than `&'static str` throughout: the record must round-trip through `Deserialize` when read back from the ledger, which a borrowed static cannot do.

**Process-global clients** — the update checker, CP's host registry — pass `run_id: None` with `Origin::Update` / `Origin::Cp` / `Origin::System`. Those flows go to the ledger only, because there is no transcript to write them to. This is the honest shape of the data and is precisely why the ledger must exist alongside the events rather than being a convenience index.

## 5. `FlowRecord`

```rust
pub struct FlowRecord {
    pub id: FlowId,                    // ULID
    pub ts: DateTime<Utc>,
    pub ctx: FlowCtx,
    pub fidelity: Fidelity,

    pub method: String,
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub path: String,                  // query-stripped — see below

    pub peer_ip: Option<IpAddr>,       // Response::remote_addr()
    pub resolved_ips: Vec<IpAddr>,     // every A/AAAA answer, from the custom resolver

    pub http_version: Option<String>,
    pub status: Option<u16>,
    pub outcome: Outcome,              // Ok | HttpError | TransportError | Timeout
    pub error: Option<String>,

    pub bytes_out: Option<u64>,
    pub bytes_in: Option<u64>,
    pub body_complete: bool,           // false while a stream is still draining

    pub ttfb_ms: Option<u64>,
    pub duration_ms: Option<u64>,
}
```

### 5.1 Deliberate omissions

- **Query strings and headers are never stored.** They routinely carry tokens. `path` is stored query-stripped. There is no opt-out in v1.
- **No TLS version or cipher.** `reqwest` does not expose it at this layer. A field that can only ever hold `None` does not belong in the schema.
- **No `asn` field.** ASN is resolved at *read* time (§6.2), not stamped at write time.

### 5.2 `Fidelity` — the honesty field

```rust
pub enum Fidelity { Coarse, Http, Full }
```

| Variant | Source | What is true |
|---|---|---|
| `Http` | instrumented `reqwest` client | exact request/response metadata |
| `Coarse` | `octocrab` | host, outcome, timing known from the connector's own code; bytes and `peer_ip` unavailable |
| `Full` | microVM (§9) | reserved; not emitted by this arc |

Every CP view renders the fidelity badge. The subsystem never claims coverage it does not have, and when the VM backend lands the same views begin showing `Full` with no change.

`octocrab` exposes `OctocrabBuilder::with_service`, so a tower layer could later lift GitHub from `Coarse` to `Http`. It is fiddly enough not to block plan 1.

## 6. ASN enrichment

### 6.1 Automatic acquisition

The operator never runs a command to get this working.

- **Source:** iptoasn.com's free combined IPv4+IPv6 prefix→ASN TSV, compacted on ingest into a sorted binary range table at `~/.rupu/netflow/asn.db`.
- **Not bundled.** The full dataset is ~7 MB compacted — meaningful bloat on a binary that already embeds `web/dist`.
- **Trigger A:** the `rupu cp serve` sweep loop (`crates/rupu-cli/src/cmd/cp.rs`, already running the gate sweep and cron tick) gains an ASN freshness tick.
- **Trigger B:** any netflow read with a missing or stale DB spawns a detached background fetch, so operators who never run `cp serve` still get enrichment.
- **Config:** a `NetflowConfig` in `crates/rupu-config` (alongside `CpConfig`) backing `[netflow].asn_auto_refresh` (default `true`) and `[netflow].asn_refresh_interval_days` (default `7` — BGP prefixes move slowly).
- **Escape hatch:** `rupu netflow update-asn --force` remains for manual refresh. It is not the primary path.
- The refresh request is itself recorded as an `Origin::System` flow, keeping the subsystem honest about its own egress.
- Failure is best-effort: log once, never block a read, never block a request.

### 6.2 Read-time enrichment

`FlowRecord` stores `peer_ip`; the CP API resolves IP→ASN when rendering. Consequences:

- A dataset that arrives late automatically improves **every historical record**. No backfill job exists or is needed.
- The write path never blocks on a file that may not be present.
- Offline and air-gapped installs degrade to "ASN data not loaded" while every other view works unchanged.

## 7. Capture mechanics

### 7.1 Middleware

`reqwest-middleware` wraps `Client` into `ClientWithMiddleware`. The type change across ~30 sites is mechanical — the request-builder API is identical.

A custom `dns_resolver` on the builder captures every A/AAAA answer for a host (`resolved_ips`), which is strictly more than the single peer actually used; `Response::remote_addr()` supplies `peer_ip`.

### 7.2 Streaming bodies — the two-phase record

`reqwest-middleware` returns a real `reqwest::Response`, so its body cannot be transparently wrapped while still returning that type. Therefore:

- When `Content-Length` is present, `bytes_in` is exact and `body_complete: true` at emit.
- For SSE streams (every provider chat path), the middleware emits at header time with `bytes_in: None, body_complete: false`. The provider's stream-consuming loop — which already counts bytes — calls `netflow::complete(flow_id, bytes_in)` to finalize.

The caller learns the `FlowId` to finalize without any ambient context: it mints the id and attaches it via `reqwest_middleware`'s `RequestBuilder::with_extension`, and the middleware uses that id rather than generating one. Explicit, no magic.

This is an explicit second choke point at roughly four call sites. The alternative is estimating the byte count, which this design does not do.

Because the record is written at header time and finalized later, the ledger is a small line enum rather than bare records — `Flow` / `Complete` / `Dropped` — and `read_flows` folds `Complete` into its matching `Flow` at read time. This keeps the ledger strictly append-only.

### 7.3 Holding the choke point

`clippy.toml` gains `disallowed-methods` entries for `reqwest::Client::new` and `reqwest::Client::builder`, permitted only inside `rupu-netflow`. A regression is a build failure, not a review miss. A repo-level test (§10) backs this up.

## 8. CP surface

### 8.1 API — `crates/rupu-cp/src/api/netflow.rs`

| Route | Returns |
|---|---|
| `GET /api/runs/:id/netflow` | flows for one run |
| `GET /api/projects/:id/netflow` | project aggregate |
| `GET /api/netflow` | global |
| `GET /api/netflow/graph?scope=…` | precomputed topology nodes + edges |

### 8.2 Web — `crates/rupu-cp/web/src/components/netflow/`

- **`NetflowGraph.tsx`** — bipartite topology. Run / step / agent nodes on the left, endpoint nodes on the right. Edges weighted by call count or bytes (toggle), colored by outcome. Reuses the `components/graph` palette tokens but **not** the DAG layout engine — this topology is bipartite, not a DAG.
- **`NetflowTable.tsx`** — sortable flow list with ASN/org column and fidelity badge.
- **`NetflowSummary.tsx`** — per-host rollup: calls, bytes in/out, p50/p95 latency, error rate.

### 8.3 Placement

Follows the rule already established for findings: **a tab on RunDetail, a project-level aggregate panel, and a global page. Never on the workflow definition** — a flow belongs to a run, not to a workflow.

### 8.4 Live Events

Flows are `Event`s, so they arrive on the run's live stream at no additional cost. They are **not** promoted to the Situation Room editorial wall by default: every LLM call is a flow, and the wall would drown. A filter toggle opts in.

## 9. Deferred: the microVM capture backend

Its own arc, gated on the guest image supply chain. It introduces the `FlowSource` port (§3.1) — being the first pull-based backend, it is what makes that abstraction earn its keep — feeds the existing `FlowSink` with `Fidelity::Full`, and adds, without schema change:

- Attribution that is definitional rather than correlated — the VM *is* the run.
- DNS visibility, including lookups that resolve and never connect (rupu is the resolver).
- Every port and protocol, raw sockets included, at the frame level.
- Optional TLS interception via a CA that exists only inside the disposable guest — URL-level rather than SNI-level forensics.
- The substrate egress *enforcement* would need, should that ever be wanted.

Exposed as a fourth launcher target alongside workspace / directory / RepoRef-clone.

## 10. Error handling

Capture must never break a request. Non-negotiable.

- The `LedgerSink` writes through a **bounded** channel to a background task. On overflow it drops and increments a counter that the UI surfaces as "N flows dropped" — visible loss, not silent loss.
- Sink or ledger write failures log once and continue.
- ASN lookup degrades to `None`; it is never fallible in a way a caller must handle.
- A panic in the middleware must not propagate into the request path.

## 11. Testing

- **Middleware:** `wiremock` server driven through the instrumented client, asserting emitted `FlowRecord` fields — including an explicit test that a query string never reaches the record.
- **Two-phase completion:** a streamed response finalizes `bytes_in` and flips `body_complete`.
- **Ledger:** write/read round-trip; `host_rollup` and `graph_view` aggregation across multiple runs.
- **ASN:** range-table lookup with boundary prefixes, an unmapped IP, and a missing-DB path.
- **Choke-point guard:** a repo-level test asserting `reqwest::Client::new` appears nowhere outside `rupu-netflow`, alongside the clippy lint.
- **CP:** API handler tests; `NetflowGraph.test.tsx`; an insta snapshot for graph layout, per the `rupu-app-canvas` precedent.

## 12. Phasing

| Plan | Content |
|---|---|
| **1** | `crates/rupu-netflow`: record, ports, ledger, ASN acquisition + read-time enrichment, client factory + middleware. Migrate `rupu-providers` (incl. the two-phase streaming completion). Land the `clippy.toml` guard. |
| **2** | Migrate the remaining sites — `rupu-scm`, `rupu-auth`, `rupu-update`, `rupu-webhook`, `rupu-cp`, `rupu-cli`. Coarse-fidelity adapter for `octocrab`. `Event::NetFlow` + live streaming. |
| **3** | CP API + views: `api/netflow.rs`, `NetflowGraph`, `NetflowTable`, `NetflowSummary`, run tab / project panel / global page. |
| *later arc* | microVM capture backend + the `FlowSource` port (§9). |
