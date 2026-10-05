# Subprocess netflow capture — bash tool connections in the network views (design)

**Status:** design, agreed in conversation 2026-10-02/04. Feasibility measured on macOS (Darwin 27) and Linux (Fedora 44, kernel 7.1).
**Extends:** `docs/superpowers/specs/2026-08-03-rupu-netflow-observability-design.md` and `docs/superpowers/specs/2026-08-04-rupu-netflow-per-run-ledger-design.md`.

## 1. Problem

The CP network views show only rupu's own egress: LLM provider and SCM connector HTTP, captured by the instrumented `reqwest` client. Connections opened by processes the agent starts through its `bash` tool (`curl`, `git`, `pip`, `npm`, scripts) never appear. The netflow spec documents this gap (§2.2) and defers it to a microVM backend (§9).

This design closes the gap by reading the operating system's own socket accounting, without changing how bash commands run.

## 2. Goal

For every connection a bash tool call's processes make, record:

- **Who** — run, agent, codename, tool-call id, and the process name + pid that owned the socket.
- **When** — opened and closed timestamps.
- **Where** — transport, remote IP:port (ASN org resolved at read time, as today), bytes in/out where the OS exposes them.

The content of the call (command, output) already lives in the transcript. Records link to the transcript by **tool-call id**, never by timestamp: transcript events carry no per-event timestamps, and concurrent calls make time matching ambiguous.

## 3. Approach: read kernel socket state

Capture reads the socket tables the operating system already maintains — the same data `nettop` (macOS) and `ss` (Linux) show. Nothing sits in the command's network path; the command's environment, privileges and performance are unchanged (the one exception is a cgroup placement on Linux, §8.2, which does not affect behaviour).

Compile-time backends: `#[cfg(target_os = "macos")]`, `#[cfg(target_os = "linux")]`, and a fallback for every other target that always reports unavailable. Release artifacts are already per-OS. Each backend runs a capability check once at startup; failure never affects the bash call and is recorded visibly (§13).

A new `Fidelity::Socket` marks these rows so the views never present them as HTTP-level captures.

## 4. Evidence (feasibility spikes, throwaway code)

| Check | macOS (`com.apple.network.statistics`, normal user) | Linux (sock_diag + per-call cgroup, normal user) |
|---|---|---|
| Subscription works unprivileged | yes | yes |
| HTTP connection attributed to the call, remote address known | yes | yes |
| Connect that never completes | yes | yes |
| Child that detached (`setsid` / double fork) | no (parent chain ends at pid 1) | yes (cgroup survives) |
| Process outside the call correctly excluded | yes | yes |
| Unconnected UDP (`sendto`) | seen, no remote address | seen, no remote address |
| Per-call spawn overhead | none (4.8 vs 4.2 ms, noise) | ~1 ms (3.1 vs 2.1 ms) delegated cgroup; 10 ms with `systemd-run` per call (rejected) |
| Watcher cost | ~0 CPU over 9 s idle; 77 µs ancestry lookup per new socket | Python full-table poll 7.8 ms per 20 ms (Rust + filtering expected far lower — must be measured, §16) |

Two findings shape the design:

- **Linux:** socket-close notifications carry no owner (no cgroup id; uid 0; inode 0), only the socket cookie and `INET_DIAG_INFO`. A dump poll includes `INET_DIAG_CGROUP_ID`. Attribution joins close events to earlier poll observations **by cookie**. A cgroup v2 directory's inode number equals its cgroup id (verified, kernel 7.1).
- **macOS:** `SRC_ADDED` arrives at socket creation while the owning process is alive, so its ancestry is resolvable then; a later `SRC_DESC` for the same `srcref` carries the remote address. Reading another process's environment is not available for Apple's platform binaries, so environment tagging is not an attribution mechanism.

## 5. Architecture

```
rupu-tools::bash ──begin(call)──▶ SubprocessCapture (port, rupu-netflow::capture)
        │ spawn /bin/sh -c <prefix?>\n<command>         ▲ implemented by
        │ spawned(pid) / finished()                      │
        ▼                                         rupu-netwatch (new crate)
   unchanged command execution                     ├─ tracker.rs   (pure attribution state machine)
                                                   ├─ linux/       (cgroup setup, sock_diag, /proc)
                                                   ├─ macos/       (ntstat client, process tree)
                                                   └─ unsupported.rs
                                                          │ FlowRecord / Complete / Capture lines
                                                          ▼
                                         the call's run sink (FanoutSink: TranscriptSink + NetflowWriter)
                                                          ▼
                                     per-run ledger  ──▶  rupu-cp /api/runs/:id/netflow, explorer
```

- **Port** lives in `rupu-netflow` (`capture` module): it owns `FlowSink` and the record model, and `rupu-tools` already depends on `FlowSink`. Hexagonal rule 1 holds — `rupu-tools` and `rupu-agent` know only the trait.
- **`rupu-netwatch`** (new lib crate) implements the port. OS-specific dependencies stay out of `rupu-netflow`, which the CP and every client crate link.
- **`rupu-runtime`** (run-assembly layer shared by CLI, orchestrator, CP) owns `net_capture::shared(&NetflowConfig) -> Option<Arc<dyn SubprocessCapture>>`, a lazily-initialised process-wide instance. One watcher per rupu process serves every concurrent run, agent and sub-agent; each call carries its own sink, so records land in the right run's ledger.
- **No `unsafe`.** The workspace forbids `unsafe_code`. Linux uses `rustix` (add its `net` feature for netlink) and `std::fs` for `/proc` and cgroupfs. macOS uses `nix` (`sys::socket::SysControlAddr` for the kernel control socket) and a safe process-info crate (`libproc`, or `sysctl` for `kinfo_proc`). If a required call has no safe wrapper, implementation stops and asks matt before adding an FFI crate with a lint override (§18, V2).

## 6. Port API (`rupu_netflow::capture`)

```rust
/// Attribution for one tool call, bound when the call begins.
pub struct CallAttribution {
    pub run_id: String,
    pub step_id: Option<String>,
    pub agent: Option<String>,
    pub codename: Option<String>,
    pub tool_call_id: String,
    /// The run's sink. Records for this call go here and nowhere else.
    pub sink: Arc<dyn FlowSink>,
}

pub trait SubprocessCapture: Send + Sync {
    /// Called by the bash tool before spawning. Never fails: an unavailable
    /// backend returns a call that does nothing (and records why, once per run).
    fn begin(&self, call: CallAttribution) -> Box<dyn CaptureCall>;
    /// Called once when a run ends (runner exit). Finalises the run's open
    /// sockets and stops attributing to its calls.
    fn run_finished(&self, run_id: &str);
}

pub trait CaptureCall: Send {
    /// Lines to prepend to the shell script (Linux cgroup entry), or None.
    fn shell_prefix(&self) -> Option<String>;
    /// The spawned shell's pid (macOS ancestry root; Linux verification).
    fn spawned(&mut self, pid: u32);
    /// The shell exited. Starts the linger window (§8.7 / §9.6).
    fn finished(self: Box<Self>);
}
```

`begin`/`finished` are synchronous and cheap (Linux: one `mkdir`; macOS: a map insert). Heavy work runs on the watcher thread.

## 7. Record model changes (`rupu-netflow`, all additive)

### 7.1 `Fidelity::Socket`
Least-observable ordering in `ledger/explorer.rs`: `Coarse < Socket < Http < Full`.

### 7.2 `Origin::Subprocess(String)`
Name = the process name (Linux: basename of `/proc/<pid>/exe`, falling back to `/proc/<pid>/comm`; macOS: descriptor `pname`). `"unknown"` when the owning process could not be resolved (§8.6). Explorer key `subprocess:<name>`, so the topology gets nodes like `subprocess:curl` beside `provider:anthropic`. Update the `Origin` doc comment and the `origin_enumerates_only_egress_that_can_occur` test: this variant can occur.

### 7.3 `FlowCtx.tool_call_id: Option<String>`
`serde(default, skip_serializing_if = "Option::is_none")`. Set only on socket flows.

### 7.4 New optional `FlowRecord` fields
- `process: Option<FlowProcess { pid: u32, name: String }>`
- `local_addr: Option<String>` — `"ip:port"`, for correlating with other logs.
- `direction: Option<Direction { Outbound, Inbound }>` — macOS from descriptor flags; Linux inbound when the local port matches a listener observed in the same call cgroup, else outbound; `None` when unknown.

### 7.5 Field mapping for socket flows
| Field | Value |
|---|---|
| `fidelity` | `Socket` |
| `scheme` | transport: `"tcp"` / `"udp"` (documented reuse) |
| `method`, `path` | `""` |
| `host` | remote IP literal (v6 unbracketed); `""` when unknown |
| `port` | remote port; `0` when unknown |
| `peer_ip` | remote IP; `None` when unknown |
| `status`, `http_version`, `ttfb_ms` | `None` |
| `outcome` | §7.7 |
| `bytes_out` / `bytes_in` | TCP: Linux `tcp_info.tcpi_bytes_acked` / `tcpi_bytes_received`; macOS `nstat_counts` tx/rx bytes. UDP: macOS counts; Linux `None` |
| `ts` | first observation time |
| `duration_ms` | at close |

### 7.6 Ledger lifecycle
- **TCP established:** `Flow` (`body_complete:false`) when first observed established; `Complete` at close.
- **TCP never established:** one `Flow` at close, `outcome: TransportError`, `error:"connection never established"`, `body_complete:true`.
- **UDP:** `Flow` when first observed; `Complete` at close.
- **Run ends, socket still open:** `Complete` with what is known, `error:"observation ended with the run; socket still open"`.

`LedgerLine::Complete` extended additively: `bytes_in` → `Option<u64>` (`serde(default)`), plus optional `bytes_out`, `outcome`, `error`. `views::read_flows*` folds the new fields. A provided `FlowSink::complete_socket(id, SocketCompletion)` is added (default no-op); `NetflowWriter`/`FanoutSink`/`MemorySink` implement it; `TranscriptSink` ignores it (transcript gets the open-time event only).

### 7.7 Outcome rules
- TCP `Ok` if established/any later state observed, or bytes received > 0 (Linux), or `connectsuccesses > 0` (macOS); else `TransportError`.
- UDP `Ok`.

### 7.8 `LedgerLine::Capture` (new variant)
```rust
Capture {
    ts: DateTime<Utc>,
    state: CaptureState,     // Active { backend } | Unavailable { reason }
    tool_call_id: Option<String>,
    note: Option<String>,    // e.g. "could not enter capture cgroup", "lost 12 kernel events"
}
```
Written once per run on first bash call, plus per-call notes for visible loss (§13). Older readers skip unknown variants, so forward compatibility holds.

## 8. Bash tool and runner integration

### 8.1 `ToolContext` (rupu-tools) gains
- `netflow_sink: Option<Arc<dyn FlowSink>>` (`#[serde(skip)]`) — the run's sink.
- `net_capture: Option<Arc<dyn SubprocessCapture>>` (`#[serde(skip)]`).
- `tool_call_id: Option<String>` — set per invocation.

### 8.2 Runner (`crates/rupu-agent/src/runner.rs`)
- At the invoke site (`tool.invoke(input.clone(), &opts.tool_context)`, ~line 1940): clone the context, set `tool_call_id = Some(call_id.clone())`, invoke with the clone. `ToolContext` is `Clone` and carries only `Arc`s plus small fields.
- At run end (next to `opts.tool_context.coverage_writer = None`, ~line 2118): `net_capture.run_finished(&run_id)` when present.

### 8.3 Wiring sites — each sets `netflow_sink` and `net_capture` on the `ToolContext` it builds
`crates/rupu-cli/src/cmd/run.rs`, `cmd/session.rs` (both `for_run` sites), `cmd/dispatch.rs` (child gets the child's own sink), `resume.rs`, `cmd/workflow.rs` (three sites), and `crates/rupu-orchestrator/src/step_factory.rs` (`step_netflow_sink`). Child contexts built in `rupu-tools/src/dispatch_agent.rs` / `dispatch_agents_parallel.rs` inherit `net_capture`; the dispatcher sets the child's sink. A test enumerates every `for_run` / `step_netflow_sink` call site and fails if a site leaves either field unset (pattern: the existing `crates/rupu-netflow/tests/it/choke_point.rs`).

### 8.4 Bash tool sequence (`crates/rupu-tools/src/bash.rs`)
1. If `net_capture`, `netflow_sink`, `run_id` and `tool_call_id` are all present: `call = capture.begin(attribution)`.
2. Build the script: if `call.shell_prefix()` is `Some(p)`, run `/bin/sh -c "<p>\n<command>"`, else `/bin/sh -c "<command>"` as today. Environment, cwd, timeout and `kill_on_drop` unchanged.
3. After spawn: `call.spawned(child.id())`.
4. After the wait (success, error or timeout): `call.finished()`.
5. `DerivedEvent::CommandRun.argv` still records the agent's original command, not the prefix.

## 9. Linux backend (`rupu-netwatch/src/linux/`)

### 9.1 Capture-root setup (once per process, lazily on first `begin`)
1. Read the `0::<path>` line of `/proc/self/cgroup`. Require cgroup2 at `/sys/fs/cgroup`, else `Unavailable("cgroup v2 not mounted")`.
2. Choose a mode, first match wins:
   - **Scope mode** — the leaf is not a `*.service` unit and a systemd manager is reachable (user manager for uid ≠ 0 via `$XDG_RUNTIME_DIR/bus`; system for uid 0). `StartTransientUnit("rupu-netwatch-<pid>-<rand>.scope","fail",[PIDs=[own pid],Delegate=true],[])` via `busctl` (no new D-Bus crate). Poll `/proc/self/cgroup` until it shows the new scope (5 ms, 2 s timeout). Root `R` = the scope.
   - **Direct mode** — the current cgroup is writable and either belongs to a unit with `Delegate=yes` or is a cgroup-namespace root. `R = <current>/rupu-netwatch-<pid>`.
   - Otherwise `Unavailable(<reason naming the cgroup and why>)`. A service without `Delegate=yes` is never modified.
3. Create `R/supervisor`, move rupu itself there (write `0` to `R/supervisor/cgroup.procs`) to satisfy cgroup v2's no-internal-processes rule. Never touch `cgroup.subtree_control`.
4. Any failing step → `Unavailable` with the errno text; partial dirs removed best-effort.

### 9.2 Per call
1. `mkdir R/call-<seq>` (`seq` process-wide; a map holds `seq → CallAttribution`). Its cgroup id is the directory's inode.
2. Shell prefix (single-quoted path, quotes escaped):
   `{ printf '%d\n' "$$" > '<R>/call-<seq>/cgroup.procs'; } 2>/dev/null`
   The shell moves itself before running anything; every descendant inherits the cgroup, including detached or privilege-changed ones. ~1 ms total, mostly spawn.
3. On `spawned(pid)`: confirm the pid is in the call cgroup (`cgroup.procs`); if not, record a `Capture` note and fall back to §9.5.

### 9.3 The watcher thread
One per process. Two netlink sources on `NETLINK_SOCK_DIAG`:
- **Close multicast** — bind groups `INET_TCP_DESTROY`, `INET_UDP_DESTROY`, `INET6_TCP_DESTROY`, `INET6_UDP_DESTROY`. Each message gives family, state, src/dst, cookie and `INET_DIAG_INFO` (final `tcp_info`). No owner.
- **Dump poll** — every `POLL_MS` (default 50; configurable), `SOCK_DIAG_BY_FAMILY` dump of TCP+UDP over v4+v6, requesting `INET_DIAG_INFO` and `INET_DIAG_CGROUP_ID`. Each live socket gives cookie → (cgroup id, state, addrs, bytes).

### 9.4 Attribution join (`tracker.rs`, pure + unit-tested)
- Poll observations update `cookie → {cgroup_id, last addrs, last bytes, states seen}`.
- A cgroup id maps to a call via the `inode(call-<seq>) → CallAttribution` map.
- A close event is looked up by cookie: if the cookie was seen in a tracked call cgroup, it is that call's; its final `tcp_info` supplies bytes and the established/never-established decision. A cookie never seen in any tracked cgroup is dropped (it is not ours).
- Sockets still live when the call's linger ends are flushed as "still open".

### 9.5 Fallback when the cgroup could not be entered
If the prefix write failed (`spawned` check), attribute by process tree instead: poll observations also carry the pid (`/proc/<pid>` owner via socket inode in `/proc/<pid>/fd`), and the tracker walks `/proc/<pid>/stat` ppid up to the shell pid from `spawned`. Lower fidelity (misses detached children) but better than nothing; the run gets a `Capture` note that cgroup mode was unavailable.

### 9.6 Linger and cleanup
`finished()` marks the call closing at `now + LINGER` (default 3 s) so sockets closing just after the shell exits are still attributed. After linger with the cgroup empty (`cgroup.procs` empty), `rmdir R/call-<seq>` (0.03 ms) and drop the map entry. A non-empty cgroup at linger end (a surviving background process) is flushed as "still open", kept until empty or `run_finished`, then removed.

### 9.7 Bytes
From `tcpi_bytes_acked` (out) and `tcpi_bytes_received` (in) in the final `tcp_info` on close; the last poll's values if the close message lacks `INET_DIAG_INFO`. UDP has no byte counts here → `None`.

## 10. macOS backend (`rupu-netwatch/src/macos/`)

### 10.1 The feed
A `PF_SYSTEM` / `SYSPROTO_CONTROL` socket connected to the kernel control named `com.apple.network.statistics` (the source behind `nettop`, usable as a normal user — verified). Subscribe to all TCP and UDP sources (`NSTAT_MSG_TYPE_ADD_ALL_SRCS` per provider, with `NSTAT_FILTER_SUPPRESS_SRC_ADDED` off and `USE_UPDATE_FOR_ADD` set so creations arrive as updates). Message handling:
- **`SRC_ADDED`** — new socket (srcref). Immediately request its description (`GET_SRC_DESC`) and resolve the owning process's ancestry **now**, while it is alive.
- **`SRC_DESC` / `SRC_UPDATE`** — carries the TCP/UDP descriptor: pid, pname, local/remote sockaddrs, state, byte counts.
- **`SRC_REMOVED`** — socket closed; finalise.

Struct layouts are ported from xnu `bsd/net/ntstat.h` rev 9 and pinned by a size/offset test (§10.4).

### 10.2 Attribution
At `SRC_ADDED`/first descriptor, walk the owning pid's parent chain via `sysctl` `kinfo_proc` (`e_ppid`), up to 32 levels. If the chain contains a shell pid registered by a live `CaptureCall` (`spawned`), the socket is that call's. The 77 µs-per-socket walk runs on the watcher thread. A socket whose chain reaches pid 1 without hitting a registered shell is not ours (unless it is the detached-child gap, §10.5).

### 10.3 Bytes, direction, lifecycle
Bytes from `nstat_counts` tx/rx. Direction from the descriptor's `NSTAT_SOURCE_IS_INBOUND`/`OUTBOUND` flags. `Flow` on first attributed descriptor, `Complete` on `SRC_REMOVED` (or run end).

### 10.4 Struct-layout safety
A test asserts `size_of`/`offset_of` for every ported struct against the rev-9 values, so a wrong offset fails CI rather than mis-parsing at runtime. A `__NSTAT_REVISION__` mismatch check logs a one-time warning but does not disable capture.

### 10.5 Known gap: detached children
A process that double-forks / `setsid`s before connecting has a parent chain ending at launchd, so it is not attributed to its call. It still appears as an `Origin::Subprocess` flow on the run (the watcher sees it on the machine while the run is active) but with `tool_call_id: None`. The run gets a one-time `Capture` note. A descendant-pre-registration sweep (record a call's live descendants before they can detach) is a possible later improvement, not in this plan.

## 11. Scope: what is and is not captured

**Captured:** TCP and UDP connections (v4/v6) made by a bash tool call's process tree, with remote IP:port, process, timing, and bytes where the OS exposes them; attributed to the run, agent and tool call.

**Not captured, by design, and stated in the UI (§12.4):**
- **Hostnames.** Only IPs are in the socket tables; DNS goes through the system resolver. The views show the IP and its ASN org, as today.
- **Unconnected UDP destinations.** `sendto` without `connect` has no socket-level peer; the row shows the transport and process but no remote address.
- **Per-datagram destinations of a single socket.** A socket that talks to many peers shows the socket, not each peer.
- **Very short-lived sockets on Linux** that open and close entirely between polls (e.g. some resolver lookups) may be unattributed. Tunable via `POLL_MS` (§14).
- **Detached children on macOS** (§10.5).
- **Remote-host runs.** Bash connections on a remote host land in that host's per-run ledger. Surfacing remote netflow in the coordinator CP is existing deferred work (the netflow transport arc), unchanged here.

## 12. CP changes

### 12.1 API (`crates/rupu-cp/src/api/netflow.rs`)
`FlowView` gains `process`, `local_addr`, `direction`, and `ctx.tool_call_id` (all optional). The existing `/api/runs/:id/netflow`, project and global endpoints serve socket flows with no routing change (they already read the per-run ledger). The explorer aggregation already groups by `origin_key`, so `subprocess:<name>` nodes appear automatically; add socket fidelity to its legend.

### 12.2 Web types (`web/src/lib/netflow.ts`)
`Fidelity` gains `'socket'`; `Origin.kind` gains `'subprocess'`; `FlowView` gains the new optional fields. Update the `Origin` doc comment (it currently lists only provider/scm/update/cp/system and explains why mcp/webhook are absent).

### 12.3 Web components
- **`FidelityBadge.tsx`** — a tone + title for `socket`: "Socket — process, remote IP:port, bytes and timing observed from the OS socket table; no URL or status."
- **`NetflowTable.tsx`** — socket rows: the `origin` column shows `curl (pid 4412)`; the `path`/`status` columns show `—`; a `network` cell shows `tcp → 140.82.116.3:443`. A "transcript" affordance when `ctx.run_id` + `ctx.tool_call_id` are present, linking to the run's transcript anchored at that call (§12.5).
- **`FlowDetailPanel.tsx`** — show process, pid, local addr, direction, bytes; hide method/path/status/TTFB for socket rows.
- **Topology / timeline / org cards** — unchanged logic; `subprocess:*` origins render as ordinary origin nodes.

### 12.4 Coverage wording (`ScopeDisclosure.tsx`)
This is the one place the covered-surface sentence lives. Add bash subprocess connections to the covered list, and the §11 gaps to the honest-limits text. When a run's ledger holds a `Capture { Unavailable }` line, surface "subprocess capture unavailable on this run: <reason>".

### 12.5 Transcript anchor
The transcript viewer needs a stable per-call anchor. `transcriptView.ts` keeps `net_flow` events deliberately unrendered (they flood the feed); that stays. Instead, `ToolCard` gets an `id` derived from `call_id`, and the network-row link scrolls to it. If adding the anchor is non-trivial, the fallback is a filter link into the per-run Netflow tab scoped to that `tool_call_id` — decided during Plan 2.

## 13. Never-break, visible-loss invariants

Inherited from the netflow spec (§10) and non-negotiable:
- **Capture never affects the bash call.** Every backend operation is best-effort; any error becomes an `Unavailable`/note, never a failure, slowdown or environment change for the command.
- **The watcher never blocks the request path.** `begin`/`finished` do only cheap synchronous work; kernel reads and attribution run on the watcher thread; records reach the ledger through the existing bounded channel, whose overflow is already counted and surfaced.
- **Loss is visible.** A dropped kernel event (socket-buffer overflow — `ENOBUFS` on macOS, a sequence gap on Linux) increments a per-run counter and emits a `Capture` note; the UI shows it. The subsystem never claims coverage it does not have.
- **Unavailability is explicit.** A backend that cannot start records exactly why, once per run, shown in `ScopeDisclosure`.

## 14. Config (`[netflow]`, `rupu-config/src/netflow_config.rs`)

All default-on where capture is possible, all overridable:
- `subprocess_capture: bool` (default `true`) — master switch.
- `subprocess_poll_ms: u64` (Linux dump-poll interval, default 50).
- `subprocess_linger_ms: u64` (default 3000).
- Env override `RUPU_NETFLOW_SUBPROCESS=0` to force off (for a host where capture misbehaves) without editing config.

## 15. Testing

- **`tracker.rs` (pure, both OSes):** unit tests over scripted observation/close sequences — attribution by cookie→cgroup (Linux) and pid-chain (macOS), established vs never-established, linger window, still-open flush, drop counting, events for untracked cookies ignored. No kernel, no sockets.
- **Record model:** round-trip the new `Fidelity`/`Origin`/fields and the extended `Complete`; a lockstep test that `read_flows` folds the new `Complete` fields; the `Origin` enumeration test updated.
- **Choke-point test:** every `for_run`/`step_netflow_sink` site sets both new `ToolContext` fields.
- **macOS struct-layout test** (§10.4).
- **Backend integration tests, `#[ignore]` by default** (need a real kernel, run on matt's hosts): a known workload (short HTTP, a connect that never completes, a detached child, an unconnected UDP send) and assertions on the resulting ledger. These are the real proof — the unit tests cover logic, not the kernel.
- **CP/web:** `NetflowTable`/`FidelityBadge`/`FlowDetailPanel`/`ScopeDisclosure` tests for socket rows and the unavailable-note path; `netflow.ts` type-level additions.

## 16. Performance budget (must be measured, not assumed)

- Linux dump poll in Rust with an `INET_DIAG` cgroup filter — measure cost per poll on an idle host and under a load of many short connections; if a full-table poll is too costly, filter the dump to the capture root's cgroup subtree (sock_diag supports a cgroup filter) rather than walking every socket. The 7.8 ms Python figure is an upper bound, not the target.
- Per-call overhead on Linux — confirm the trimmed prefix (no extra `exec`) is ≤ ~1 ms.
- macOS watcher CPU under a run doing heavy networking — the idle figure (~0) is not enough; measure under load.
- A regression guard: capturing vs not on a fixed bash-heavy workload, asserting wall-time within a small bound.

## 17. Plan decomposition (for writing-plans)

1. **Record model + port.** `Fidelity::Socket`, `Origin::Subprocess`, `FlowCtx.tool_call_id`, the new `FlowRecord` fields, the extended `Complete` + `complete_socket`, `LedgerLine::Capture`, the `capture` port module, and all serde/fold/enumeration tests. Pure `rupu-netflow`; no OS code, no behaviour change. Ships independently.
2. **`rupu-netwatch` + tracker.** The crate, the pure `tracker.rs` with its unit tests, the `unsupported.rs` backend, and `rupu-runtime::net_capture::shared`. No bash wiring yet.
3. **Linux backend.** cgroup setup, netlink sources, poll, the `#[ignore]` integration test on tachikoma, the perf measurements (§16).
4. **macOS backend.** ntstat client, ancestry attribution, struct-layout test, the `#[ignore]` integration test on this Mac / Sutajio.
5. **Bash + runner + wiring + config.** `ToolContext` fields, the bash sequence, the runner tool-call-id and run-finished hooks, every wiring site, the choke-point test, `[netflow]` config. End to end on the CLI.
6. **CP API + web.** `FlowView` fields, the web types, the four components, the coverage wording, the transcript anchor (or its fallback). Visual check before merge (matt runs the CP).

Each plan is independently mergeable and leaves the tree green; capture produces nothing user-visible until Plan 5, and the views light up in Plan 6.

## 18. Open questions for the plan author

- **V1 — Linux listener direction.** Marking a socket inbound by matching a listener in the same call cgroup (§7.4) needs the poll to track listeners too. If that adds cost, drop `direction` on Linux to `None` for v1 and revisit.
- **V2 — safe syscall wrappers.** Confirm `rustix` exposes netlink send/recv for `NETLINK_SOCK_DIAG`, and that `nix`'s `SysControlAddr` + `libproc` cover the macOS feed and ancestry with no `unsafe`. If a gap forces FFI, stop and ask matt before adding a crate with a lint override — the workspace forbids `unsafe_code`.
- **V3 — poll vs event-only on Linux.** The dump poll exists only to attach a cgroup id to each cookie before close. If a cheaper mechanism (a BPF-free sock_diag cgroup filter, or `SOCK_DESTROY` introspection) can carry the cgroup on the close message, the poll can be dropped. Investigate in Plan 3; the tracker design already isolates the join so the source can change.

## 19. Relationship to the deferred microVM backend

This is complementary, not a replacement. The microVM (netflow §9) would give definitional attribution, DNS visibility, every protocol at the frame level and `Fidelity::Full`, at the cost of the guest-image supply chain. Subprocess capture gives `Fidelity::Socket` now, on both OSes, with no execution-model change. When the microVM lands, its `Full` flows and these `Socket` flows coexist in the same ledger and views; the fidelity badge keeps them honest.
